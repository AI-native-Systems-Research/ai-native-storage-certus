# Implementation Plan: Memory Tier

**Branch**: `001-memory-tier` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The memory-tier component provides a DRAM-resident cache pool for the Certus storage system. It manages a single contiguous memory region (SPDK hugepages or mmap fallback) using a first-fit free-list allocator with 4 KiB alignment. The pool is sharded into 16 independent partitions keyed by `CacheKey % 16` to minimize lock contention. Eviction ordering is delegated to an external `IEvictionPolicy` receptacle (LRU). The component sits between the block-device layer and the dispatcher, serving as a fast staging area for data that avoids round-trips to NVMe.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**: `component-framework` (COM model), `interfaces` crate (trait definitions), `libc` (mmap/mbind), `spdk-sys` (optional, feature-gated DMA allocation)
**Storage**: In-memory only (contiguous pool allocated at initialization)
**Testing**: `cargo test -p memory-tier` (unit tests in `src/lib.rs` and `src/allocator.rs`)
**Target Platform**: Linux only (RHEL/Fedora, hugepages + IOMMU for SPDK path)
**Project Type**: Library component (provides `IMemoryTier` interface)
**Performance Goals**: Zero system calls on the data path after initialization; per-shard mutex for O(1) amortized insert/get; first-fit allocation with BTreeMap for O(log n) free-region lookup
**Constraints**: 4 KiB alignment for NVMe DMA compatibility; all pointers valid until evict/remove; pool memory bound to a NUMA node when specified
**Scale/Scope**: Default 256 MiB pool; 16 shards; designed for thousands of concurrent cache entries accessed by multiple dispatcher threads

## Architecture

### Component Layer

```
 ┌───────────────────────────────────────────────────────────────┐
 │                    Dispatcher Threads                          │
 │         (insert / get / evict_lru / touch / remove)           │
 └────────────────────────────┬──────────────────────────────────┘
                              │ IMemoryTier
 ┌────────────────────────────▼──────────────────────────────────┐
 │                   MemoryTierComponent                          │
 │  ┌──────────────────────────────────────────────────────────┐ │
 │  │                  RwLock<MemoryTierState>                  │ │
 │  │  ┌────────┐ ┌────────┐ ┌────────┐       ┌────────┐     │ │
 │  │  │Shard 0 │ │Shard 1 │ │Shard 2 │  ...  │Shard 15│     │ │
 │  │  │Mutex   │ │Mutex   │ │Mutex   │       │Mutex   │     │ │
 │  │  │        │ │        │ │        │       │        │     │ │
 │  │  │FreeList│ │FreeList│ │FreeList│       │FreeList│     │ │
 │  │  │HashMap │ │HashMap │ │HashMap │       │HashMap │     │ │
 │  │  └────────┘ └────────┘ └────────┘       └────────┘     │ │
 │  │                                                          │ │
 │  │  pool_ptr ──► [ contiguous memory region, pool_size ]    │ │
 │  │               [ shard_size = pool_size / 16 per shard ]  │ │
 │  └──────────────────────────────────────────────────────────┘ │
 │                                                                │
 │  Receptacles:                                                  │
 │    eviction_policy: IEvictionPolicy ──► EvictionPolicyLru      │
 │    logger: ILogger (optional)                                  │
 └────────────────────────────────────────────────────────────────┘
                              │
              ┌───────────────┼───────────────┐
              │               │               │
              ▼               ▼               ▼
       IEvictionPolicy     libc           spdk-sys
       (LRU tracking)   (mmap/mbind)   (spdk_zmalloc)
```

### Internal Module Structure

```
components/memory-tier/
├── Cargo.toml                    # Package manifest; features: ["spdk"]
├── src/
│   ├── lib.rs                    # Component definition, IMemoryTier impl, tests
│   └── allocator.rs              # FreeList: BTreeMap first-fit allocator with coalescing
├── .specify/
│   └── specs/001-memory-tier/
│       ├── spec.md               # Feature specification (backfilled)
│       ├── plan.md               # This file
│       └── tasks.md              # Task list for review
└── README.md                     # Component documentation
```

### Data Flow

#### Insert Flow (`insert(key, size)`)

```
1. Validate: size > 0, state.initialized == true
2. Acquire state RwLock (read)
3. Compute shard_idx = key % 16
4. Lock shard[shard_idx].mutex
5. Check: key not already in shard.slots (reject AlreadyExists)
6. FreeList::allocate(size) -> local_offset (reject PoolFull if None)
7. IEvictionPolicy::track(pool_id, key) -> EvictionHandle
8. Insert Slot { offset, size, eviction_handle } into shard.slots
9. Compute global pointer: pool_ptr + (shard_idx * shard_size) + local_offset
10. Return pointer to caller
```

#### Get Flow (`get(key)`)

```
1. Acquire state RwLock (read)
2. Compute shard_idx = key % 16
3. Lock shard[shard_idx].mutex
4. Lookup key in shard.slots -> Slot { offset, size, eviction_handle }
5. Compute global pointer: pool_ptr + (shard_idx * shard_size) + offset
6. Drop shard lock
7. IEvictionPolicy::touch(eviction_handle)  [LRU promotion]
8. Return (pointer, size)
```

#### Evict Flow (`evict_lru()`)

```
1. Acquire state RwLock (read)
2. start = evict_counter.fetch_add(1, Relaxed) % 16
3. For i in 0..16:
   a. idx = (start + i) % 16
   b. IEvictionPolicy::pop_oldest(pool_ids[idx]) -> Some(key)?
   c. Lock shard[idx].mutex
   d. Remove key from shard.slots -> Slot
   e. FreeList::deallocate(offset, size)
   f. Return evicted key
4. Return None (all shards empty)
```

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **16 fixed shards (key % 16)** | Balances contention reduction against memory overhead. 16 is a power of 2 matching typical core counts. Simple modulo assumes uniform key distribution. |
| **Per-shard Mutex (not RwLock)** | Data-path operations (insert, get, remove) all mutate either the FreeList or the HashMap; read-only access is rare. Mutex avoids reader-writer fairness issues. |
| **BTreeMap first-fit allocator** | O(log n) lookup for first region >= requested size. BTreeMap ordered iteration enables coalescing adjacent regions on deallocation without a separate pass. |
| **4 KiB alignment** | Matches NVMe sector size. Returned pointers are DMA-capable without intermediate alignment copies. Trades internal fragmentation for zero-copy I/O compatibility. |
| **External eviction policy (receptacle)** | Decouples ordering strategy from memory management. Allows swapping LRU for LFU/ARC without changing memory-tier code. Each shard gets an independent pool_id in the eviction policy. |
| **RwLock for top-level state** | Only `initialize()` takes the write lock. All data-path operations hold only a read lock on the outer state, then a per-shard Mutex. This avoids a global bottleneck. |
| **mmap with MAP_HUGETLB fallback** | Hugepages reduce TLB pressure for large pools. Graceful fallback to anonymous pages when hugepages are unavailable. |
| **SPDK allocation when active** | `spdk_zmalloc` provides IOMMU-registered memory suitable for zero-copy DMA. Controlled by the `spdk` feature flag at compile time plus runtime `is_spdk_env_active()` check. |
| **Atomic round-robin eviction** | `AtomicUsize` with `Relaxed` ordering provides approximate fairness across shards without locking. Exact fairness is not required for eviction. |
| **Intentional leak on SPDK shutdown** | If SPDK env has already been torn down, calling `spdk_free` would use-after-free. Leaking is safe and the process is terminating anyway. |

## Dependencies

| Crate | Version | Role | Feature Gate |
|-------|---------|------|--------------|
| `component-framework` | workspace | `define_component!` macro, receptacle wiring | always |
| `component-core` | workspace | `IUnknown`, `query_interface!` | always |
| `component-macros` | workspace | Proc macros for interface/component definitions | always |
| `interfaces` | workspace | `IMemoryTier`, `IEvictionPolicy`, `ILogger`, `CacheKey`, `EvictionHandle`, `PoolId`, `MemoryTierError` | features = ["spdk"] |
| `libc` | 0.2 | `mmap`, `munmap`, `mbind` (SYS_mbind), `MAP_HUGETLB` | always |
| `spdk-sys` | workspace | `spdk_zmalloc`, `spdk_free` for DMA-capable allocation | optional, `features = ["spdk"]` |
| `eviction-policy-lru` | workspace | Concrete LRU implementation wired via receptacle | dev-dependencies only |

## Testing

### Existing Unit Tests (in `src/lib.rs`)

| Test | Coverage |
|------|----------|
| `initialize_twice_fails` | FR-001 double-init error |
| `insert_and_get` | FR-005, FR-006 basic path |
| `insert_duplicate_fails` | FR-005 AlreadyExists |
| `insert_zero_size_fails` | FR-005 InvalidSize |
| `remove_and_reuse` | FR-010 remove + re-insert |
| `evict_lru_returns_some` | FR-008 basic eviction |
| `pool_full_returns_error` | FR-005 PoolFull |
| `capacity_and_used` | FR-014 accounting |
| `contains` | FR-013 presence check |
| `clear_resets_all` | FR-017 full reset |
| `touch_updates_lru` | FR-011 LRU promotion |
| `peek_does_not_update_lru` | FR-007 non-promoting read |

### Existing Allocator Tests (in `src/allocator.rs`)

| Test | Coverage |
|------|----------|
| `allocate_single` | Basic allocation |
| `allocate_rounds_up` | 4 KiB alignment rounding |
| `allocate_sequential` | Sequential offset progression |
| `allocate_fails_when_full` | Capacity exhaustion |
| `deallocate_and_reuse` | Free and reallocate |
| `coalesce_adjacent` | NFR-003 forward coalescing |
| `coalesce_with_following` | NFR-003 backward coalescing |
| `zero_size_returns_none` | Zero-size rejection |
| `capacity_tracking` | Capacity/used accounting |

### Formal Verification (Creusot)

10 properties verified with 21 verification conditions discharged by SMT solvers:

| Property | Statement |
|----------|-----------|
| P1 | `insert` rejects `size == 0` |
| P2 | All operations fail when pool not initialized |
| P3 | `insert` rejects key that already has a slot |
| P4 | `shard_for_key` always returns index < 16 |
| P5 | Same key always maps to same shard |
| P6 | `insert` increases used by size; `remove` decreases by size |
| P7 | `used()` never exceeds `capacity()` |
| P8 | `insert` returns PoolFull when allocator cannot satisfy |
| P9 | `remove` on absent key returns KeyNotFound |
| P10 | `evict_lru` cycles through all 16 shards |

### Testing Gaps Identified

- No concurrent stress test (multi-threaded insert/evict races)
- No NUMA binding verification (requires multi-socket hardware)
- No SPDK-path integration test (requires SPDK environment)
- No fragmentation stress test (many small alloc/dealloc cycles)
- No `evict_lru_for_key` targeted eviction test
- No `batch_touch` test
- No `oldest_keys` test
- No property-based/fuzz tests for allocator

## Future Considerations

1. **Dynamic shard count**: Currently fixed at 16 (compile-time). A runtime-configurable shard count (power of 2) would allow tuning for different core counts.

2. **Better key distribution**: Simple modulo can cause hot shards with clustered keys. A hash-based shard selection (e.g., `key.wrapping_mul(GOLDEN_RATIO) >> (64 - 4)`) would improve uniformity.

3. **Tiered allocation sizes**: The 4 KiB minimum alignment wastes space for small metadata objects. A separate small-object pool (slab allocator) could reduce internal fragmentation for sub-4K entries.

4. **Eviction policy hot-swapping**: The receptacle model already supports this, but there is no runtime API to swap eviction policies without clearing the pool.

5. **Metrics and observability**: Per-shard hit/miss/eviction counters would enable monitoring cache effectiveness and detecting hot shards.

6. **Write-through integration**: The `NotEvictable` error variant exists but is unused. Future write-through support would mark dirty entries as non-evictable until flushed.

7. **GPU/CUDA host registration**: `pool_info()` returns the base pointer for CUDA host registration (`cudaHostRegister`). A future enhancement could register automatically when a GPU receptacle is connected.

8. **Loom testing**: Concurrent initialization race (double-init) and concurrent evict/insert/remove interactions would benefit from Loom model-checking.
