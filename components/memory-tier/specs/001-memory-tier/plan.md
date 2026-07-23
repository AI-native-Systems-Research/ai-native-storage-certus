# Implementation Plan: Memory Tier (DRAM Cache Pool)

**Branch**: `001-memory-tier` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation.

## Summary

The memory-tier component implements a sharded DRAM cache pool with pluggable eviction policy. The pool is pre-allocated at initialization time (via mmap or SPDK hugepages) and divided into 16 shards. Each shard maintains a first-fit free-list allocator and a HashMap of active slots. Eviction decisions are delegated to an external IEvictionPolicy receptacle (currently backed by eviction-policy-lru). The component exposes the IMemoryTier interface for insert/get/remove/evict operations.

## Technical Context

### Component Model Integration

The component is defined using the `define_component!` macro, which generates:
- `IUnknown` implementation for runtime interface discovery
- Receptacle slots for `ILogger` and `IEvictionPolicy`
- Arc-based ownership with interior mutability via RwLock/Mutex

### Memory Layout

```
Pool (contiguous mmap or spdk_zmalloc region):
+------------------+------------------+-----+-------------------+
| Shard 0          | Shard 1          | ... | Shard 15          |
| (pool_size / 16) | (pool_size / 16) |     | (pool_size / 16)  |
+------------------+------------------+-----+-------------------+

Each shard managed by independent FreeList allocator.
Shard assignment: key % 16
```

### Pointer Arithmetic

Global offset for a key in shard S with local offset L:
```
global_offset = S * shard_size + L
ptr = pool_ptr + global_offset
```

### Concurrency Model

```
RwLock<MemoryTierState>
  |
  +-- Read-lock on all data-path operations (hot path)
  +-- Write-lock only during initialize() (cold path, once)
  |
  +-- Per-shard Mutex<Shard> for fine-grained write access
       |
       +-- allocator: FreeList (mutated on insert/remove/evict)
       +-- slots: HashMap<CacheKey, Slot> (mutated on insert/remove/evict)
```

## Architecture

### Source Files

| File | Responsibility |
|------|---------------|
| `src/lib.rs` | Component definition, IMemoryTier implementation, initialization, sharding logic, unit tests |
| `src/allocator.rs` | FreeList allocator: first-fit allocation, deallocation with coalescing, capacity tracking |

### Key Design Decisions

1. **Sharding (16-way)**: Reduces lock contention. Key-to-shard is deterministic (modulo), so operations on distinct keys can proceed in parallel across shards.

2. **Contiguous pool**: A single mmap region avoids per-allocation syscalls. Internal fragmentation is managed by the free-list with coalescing.

3. **4 KiB alignment**: Matches NVMe sector size and page size. All allocations round up to this alignment, ensuring DMA compatibility.

4. **External eviction policy**: Decouples eviction strategy from the memory pool. The memory-tier only knows about EvictionHandle values; ordering decisions are made by the policy.

5. **SPDK optional feature**: The SPDK allocation path is compile-time gated. Without the feature, the component uses mmap. With it, SPDK hugepages are preferred when the SPDK env is active at runtime.

6. **Round-robin eviction**: The `evict_counter` atomic ensures global eviction cycles through all shards rather than always targeting the first.

### Error Handling Strategy

All error cases are captured in the `MemoryTierError` enum:
- `InvalidSize` - zero-size insert
- `AlreadyExists(key)` - duplicate key insertion
- `PoolFull` - allocator cannot satisfy request
- `KeyNotFound(key)` - remove/lookup of absent key
- `AllocationFailed(msg)` - mmap or SPDK allocation failure
- `NotInitialized(msg)` - operation before initialize()
- `NotEvictable(key)` - reserved for future write-through protection
- `DEFAULT_POOL_SIZE` (256 MiB) - public constant, reserved for a future default-constructor path; not consumed by any call site today (`initialize()` always takes an explicit `pool_size`) *(backfilled 2026-07-22)*

## Dependencies

### Required Receptacles

| Receptacle | Interface | Binding Time | Notes |
|------------|-----------|--------------|-------|
| `eviction_policy` | `IEvictionPolicy` | Before `initialize()` | Must be connected before init; verified at init time |
| `logger` | `ILogger` | Optional, any time | Best-effort logging; operations succeed without it |

### Crate Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `component-framework` | workspace | Facade re-export |
| `component-core` | workspace | IUnknown, query_interface |
| `component-macros` | workspace | define_component!, define_interface! |
| `interfaces` | workspace (spdk feature) | IMemoryTier, IEvictionPolicy, ILogger type defs |
| `libc` | 0.2 | mmap, munmap, mbind, MAP_HUGETLB |
| `spdk-sys` | workspace (optional) | spdk_zmalloc, spdk_free |

## Testing

### Unit Tests (src/lib.rs)

| Test | Covers |
|------|--------|
| `initialize_twice_fails` | Double-init rejection |
| `insert_and_get` | Basic insert + get round-trip |
| `insert_duplicate_fails` | AlreadyExists error |
| `insert_zero_size_fails` | InvalidSize error |
| `remove_and_reuse` | Remove frees slot, allows re-insert |
| `evict_lru_returns_some` | Eviction produces a key |
| `pool_full_returns_error` | PoolFull when shard exhausted |
| `capacity_and_used` | Accounting correctness |
| `contains` | Presence check |
| `clear_resets_all` | Full cache clear |
| `touch_updates_lru` | Touch promotes entry, eviction respects order |
| `peek_does_not_update_lru` | Peek does not promote |

### Unit Tests (src/allocator.rs)

| Test | Covers |
|------|--------|
| `allocate_single` | Basic allocation |
| `allocate_rounds_up` | Sub-4KiB rounds to 4096 |
| `allocate_sequential` | Sequential offset progression |
| `allocate_fails_when_full` | Full allocator returns None |
| `deallocate_and_reuse` | Freed space is reusable |
| `coalesce_adjacent` | Forward coalescing |
| `coalesce_with_following` | Backward + forward coalescing |
| `zero_size_returns_none` | Zero-size allocation rejection |
| `capacity_tracking` | Capacity and used bookkeeping |

### Formal Verification (Creusot)

10 properties verified with 21 SMT-discharged verification conditions:
- P1: size-nonzero
- P2: init-guard
- P3: no-duplicates
- P4: shard-bounded
- P5: shard-deterministic
- P6: capacity-accounting
- P7: used-within-capacity
- P8: pool-full
- P9: remove-key-not-found
- P10: evict-round-robin

### Test Gaps Identified

- No concurrency test (multi-threaded insert/evict)
- No stress test for allocator fragmentation
- No integration test with real SPDK environment
- No test for NUMA binding (requires multi-socket hardware)
- No test for batch_touch behavior
- No benchmark for throughput characterization

## Future Considerations

1. **Criterion benchmarks**: No benchmarks are currently defined. Adding throughput benchmarks for insert/get/evict under contention would validate the sharding strategy.

2. **Adaptive shard count**: Currently hard-coded to 16. Could be configurable based on hardware thread count.

3. **Write-through tracking**: The `NotEvictable` error variant exists but is unused. A future enhancement could pin entries during write-through to prevent eviction of dirty data.

4. **Statistics / metrics**: The component logs via ILogger but does not expose structured metrics (hit rate, eviction rate, fragmentation ratio).

5. **NUMA-aware sharding**: Currently shards are assigned by key modulo. A NUMA-aware scheme could map shards to NUMA nodes for locality.

6. **Large allocation support**: The free-list first-fit strategy may fragment under mixed-size workloads. A buddy allocator or size-class approach could improve utilization.

7. **GPU integration**: `pool_info()` exposes the base pointer for CUDA host registration, but no cuMemHostRegister call is made within this component.

## Spec-Sync Notes (2026-07-22)

- An optional `telemetry` Cargo feature (eviction count, read/write lock-contention counters, exposed via `telemetry()`/`reset_telemetry()`/`telemetry_snapshot()`) exists in `src/lib.rs` and is now captured in `spec.md` FR-027/FR-028/NFR-011 *(backfilled)*.
- The 16-way sharding design described throughout this plan (Memory Layout, Pointer Arithmetic, Concurrency Model, Key Design Decision #1 and #6) does **not** match the current single-pool implementation. This plan has been left as-is (describing the original sharded design) rather than rewritten, pending a decision on whether sharding is unfinished work or was intentionally dropped. See `.specify/sync/align-tasks.md` ("sharding-not-implemented") before treating this plan's architecture sections as current.
