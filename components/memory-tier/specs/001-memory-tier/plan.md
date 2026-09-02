# Implementation Plan: Memory Tier (DRAM Cache Pool)

**Branch**: `001-memory-tier` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation.
**Last-Synced**: 2026-09-02 (spec-sync: backfilled the architecture sections to the single-`RwLock<Pool>` reality, matching `spec.md`; removed the never-built 16-way sharding and Creusot P1–P10 material — see "Spec-Sync Notes" at end).

## Summary

The memory-tier component implements a DRAM cache pool with a pluggable eviction policy. The pool is pre-allocated at initialization time (via mmap or SPDK hugepages) and held as a single, unsharded structure: one first-fit free-list allocator and one `HashMap<CacheKey, Slot>` slot map, guarded by a single `RwLock<Pool>`. Eviction decisions are delegated to an external `IEvictionPolicy` receptacle (currently backed by eviction-policy-lru). The component exposes the `IMemoryTier` interface for insert/get/remove/evict operations.

## Technical Context

### Component Model Integration

The component is defined using the `define_component!` macro, which generates:
- `IUnknown` implementation for runtime interface discovery
- Receptacle slots for `ILogger` and `IEvictionPolicy`
- Arc-based ownership with interior mutability via RwLock

### Memory Layout

```
Pool (single contiguous mmap or spdk_zmalloc region):
+-----------------------------------------------------------+
| One first-fit FreeList allocator over the whole region    |
| One HashMap<CacheKey, Slot> slot map                      |
| Both guarded by a single RwLock<Pool>                     |
+-----------------------------------------------------------+
```

There is no sharding: the whole pool is one allocator plus one slot map. Freed
space is globally allocatable regardless of which key freed it.

### Pointer Arithmetic

A slot records a byte `offset` into the pool. The pointer for a slot is:
```
ptr = pool_ptr + slot.offset
```

### Concurrency Model

```
RwLock<MemoryTierState>
  |
  +-- Read-lock on all data-path operations; write-lock only during initialize()
  |
  +-- Inner RwLock<Pool> guards the single allocator + slot map:
       |
       +-- read lock:  get, peek, contains, batch_touch, capacity, used
       +-- write lock: insert, remove, evict, clear
       |
       +-- allocator: FreeList (mutated on insert/remove/evict/clear)
       +-- slots: HashMap<CacheKey, Slot> (mutated on insert/remove/evict/clear)

Eviction-order touches are applied AFTER releasing the inner pool lock; the
bound IEvictionPolicy has its own internal synchronization.
```

## Architecture

### Source Files

| File | Responsibility |
|------|---------------|
| `src/lib.rs` | Component definition, IMemoryTier implementation, initialization, unit tests |
| `src/allocator.rs` | FreeList allocator: first-fit allocation, deallocation with coalescing, capacity tracking |

### Key Design Decisions

1. **Single unsharded pool**: The pool is one `FreeList` allocator plus one `HashMap<CacheKey, Slot>` behind a single `RwLock<Pool>`. Readers (`get`/`peek`/`contains`/`batch_touch`/`capacity`/`used`) share a read lock; mutators (`insert`/`remove`/`evict`/`clear`) take the write lock. This keeps the allocator simple and correct; freed space is globally reusable.

2. **Contiguous pool**: A single mmap region avoids per-allocation syscalls. Internal fragmentation is managed by the free-list with coalescing.

3. **4 KiB alignment**: Matches NVMe sector size and page size. All allocations round up to this alignment, ensuring DMA compatibility.

4. **External eviction policy**: Decouples eviction strategy from the memory pool. The memory-tier only knows about EvictionHandle values; ordering decisions are made by the policy.

5. **SPDK optional feature**: The SPDK allocation path is compile-time gated. Without the feature, the component uses mmap. With it, SPDK hugepages are preferred when the SPDK env is active at runtime.

6. **Delegated victim selection**: `evict_next()` delegates the choice of victim entirely to the bound `IEvictionPolicy` (`identify_next_to_evict(pool_id)`), then removes the returned slot and frees its allocation. There is no internal round-robin counter or shard-selection state. `evict_next_for_key(key)` is an alias for `evict_next()` — the `key` argument is ignored because the pool is not sharded.

### Error Handling Strategy

All error cases are captured in the `MemoryTierError` enum:
- `InvalidSize` - zero-size insert
- `AlreadyExists(key)` - duplicate key insertion
- `PoolFull` - allocator cannot satisfy request
- `KeyNotFound(key)` - remove/lookup of absent key
- `AllocationFailed(msg)` - mmap or SPDK allocation failure
- `NotInitialized(msg)` - operation before initialize()
- `NotEvictable(key)` - reserved for future write-through protection; not returned by any code path today
- `DEFAULT_POOL_SIZE` (256 MiB) - public constant, reserved for a future default-constructor path; not consumed by any call site today (`initialize()` always takes an explicit `pool_size`)

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
| `evict_next_returns_some` | Eviction produces a key |
| `pool_full_returns_error` | PoolFull when pool capacity is exhausted |
| `capacity_and_used` | Accounting correctness |
| `contains` | Presence check |
| `clear_resets_all` | Full cache clear |
| `touch_updates_eviction_order` | Touch promotes entry, eviction respects order |
| `peek_does_not_update_eviction_order` | Peek does not promote |

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

### Formal Verification

None. No Creusot (or other) proof artifacts exist for this component; there is no
`components/memory-tier/verif/` directory. Earlier revisions of this plan claimed
"10 properties verified with Creusot (21 verification conditions)" for a sharded
design that was never built; that claim has been removed as part of the backfill.

### Test Gaps Identified

- No concurrency test (multi-threaded insert/evict)
- No stress test for allocator fragmentation
- No integration test with real SPDK environment
- No test for NUMA binding (requires multi-socket hardware)
- No test for batch_touch behavior
- No benchmark for throughput characterization

## Future Considerations

1. **Criterion benchmarks**: No benchmarks are currently defined. Adding throughput benchmarks for insert/get/evict under contention would characterize the single-lock design and reveal whether finer-grained locking is warranted.

2. **Finer-grained locking**: The pool is currently a single `RwLock<Pool>`. If write-lock contention becomes a bottleneck under high insert/evict rates, a sharded or lock-striped allocator could be revisited. (An earlier design proposed 16-way sharding; it was never built.)

3. **Write-through tracking**: The `NotEvictable` error variant exists but is unused. A future enhancement could pin entries during write-through to prevent eviction of dirty data.

4. **Statistics / metrics**: Beyond the optional `telemetry` feature (eviction and lock-contention counters), the component does not expose richer structured metrics (hit rate, fragmentation ratio).

5. **NUMA-aware allocation**: The pool is bound to a single NUMA node at init via `mbind`. A future scheme could partition the pool across NUMA nodes for multi-socket locality.

6. **Large allocation support**: The free-list first-fit strategy may fragment under mixed-size workloads. A buddy allocator or size-class approach could improve utilization.

7. **GPU integration**: `pool_info()` exposes the base pointer for CUDA host registration, but no cuMemHostRegister call is made within this component.

## Spec-Sync Notes (2026-09-02 — backfilled to reality)

- This plan previously described a 16-way sharded pool (Memory Layout, Pointer Arithmetic, Concurrency Model, Key Design Decisions #1 and #6) and a "Formal Verification (Creusot)" section listing properties P1–P10. Neither the sharding nor the proofs were ever built. The 2026-08-20 Phase B decision resolved the sharding fate by backfilling `spec.md` to the single-`RwLock<Pool>` implementation; this pass brings `plan.md` into line with that resolved decision. The architecture sections now describe the shipped single-pool design, and the Creusot section is removed.
- The optional `telemetry` Cargo feature (eviction count, read/write lock-contention counters, exposed via `telemetry()`/`reset_telemetry()`/`telemetry_snapshot()`) is captured in `spec.md` FR-027/FR-028/NFR-011.
- **Open (HUMAN_DECISION):** the component version disagrees across three locations — `Cargo.toml` = `0.1.0`, `define_component!` macro = `0.3.0` (`src/lib.rs:140`), `spec.md` NFR-008 = `0.2.0`. Reconciling requires editing `Cargo.toml` and `src/lib.rs`, which are out of spec-sync edit scope. See `.specify/sync/align-tasks.md`.
