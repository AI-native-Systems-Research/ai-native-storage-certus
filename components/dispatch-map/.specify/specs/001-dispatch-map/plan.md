# Implementation Plan: Dispatch Map

**Branch**: `001-dispatch-map` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The dispatch-map component is a thread-safe, in-memory routing table that maps extent keys (`CacheKey = u64`) to their physical location (DRAM memory-tier pointer or block-device byte offset). It uses per-entry readers-writer reference counting with condvar-based blocking and 2-second timeouts to coordinate concurrent access without data races. The component integrates with the Certus eviction policy for LRU ordering and with the extent manager for crash recovery.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `component-framework` (workspace) — `define_component!` macro, component wiring infrastructure
- `component-core` (workspace) — `IUnknown` trait, `query_interface!` macro
- `component-macros` (workspace) — Procedural macros for `define_interface!` / `define_component!`
- `interfaces` (workspace, feature `spdk`) — Trait definitions: `IDispatchMap`, `ILogger`, `IExtentManager`, `IEvictionPolicy`
- `eviction-policy-lru` (workspace, dev-dependency) — Concrete LRU implementation for tests/benchmarks
- `criterion 0.5` (dev-dependency) — Benchmark framework

## Architecture

### Component Layer

```
┌─────────────────────────────────────────────────────────┐
│                  Certus Dispatcher                       │
│                 (consumer of IDispatchMap)               │
└─────────────────────┬───────────────────────────────────┘
                      │ IDispatchMap
┌─────────────────────▼───────────────────────────────────┐
│              DispatchMapComponent                        │
│  ┌──────────────────────────────────────────────────┐   │
│  │  DispatchMapState                                │   │
│  │  ┌────────────────┐  ┌────────┐                  │   │
│  │  │ Mutex<Inner>   │  │Condvar │                  │   │
│  │  │ ┌────────────┐ │  └────────┘                  │   │
│  │  │ │ HashMap    │ │  ┌────────────────────────┐  │   │
│  │  │ │ CacheKey → │ │  │ Mutex<Option<PoolId>> │  │   │
│  │  │ │  Dispatch  │ │  └────────────────────────┘  │   │
│  │  │ │   Entry    │ │                              │   │
│  │  │ └────────────┘ │                              │   │
│  │  └────────────────┘                              │   │
│  └──────────────────────────────────────────────────┘   │
│                                                         │
│  Receptacles:                                           │
│    logger: ILogger (optional)                           │
│    extent_manager: IExtentManager (optional)            │
│    eviction_policy: IEvictionPolicy (required)          │
└───────────┬──────────────────┬──────────────┬───────────┘
            │                  │              │
   ┌────────▼────┐   ┌────────▼────┐  ┌──────▼──────────┐
   │  ILogger    │   │IExtentMgr   │  │IEvictionPolicy  │
   │ (optional)  │   │ (optional)  │  │ (required)      │
   └─────────────┘   └─────────────┘  └─────────────────┘
```

### Internal Module Structure

```
components/dispatch-map/
├── Cargo.toml                          # Package manifest (v0.1.0)
├── CLAUDE.md                           # Component-level AI guidance
├── README.md                           # Component documentation
├── src/
│   ├── lib.rs                          # Component definition, IDispatchMap impl, unit tests
│   ├── entry.rs                        # DispatchEntry struct, Location enum, Send+Sync impls
│   └── state.rs                        # DispatchMapState, Inner, wait_for() blocking logic
├── tests/
│   └── integration.rs                  # Multi-threaded tests, mock IExtentManager, recovery
└── benches/
    └── dispatch_map_benchmark.rs       # Criterion: lookup, ref ops, LRU, entry size
```

### Data Flow

**Lookup (hot path)**:
1. `wait_for(2s)` — condvar loop until `write_ref == 0` for key, or timeout
2. Lock `Mutex<Inner>`, find entry in HashMap
3. If writer still active after timeout: return `Err(Timeout)`
4. Increment `read_ref`, copy `eviction_handle`
5. Build `LookupResult` from `Location` variant
6. Drop mutex lock
7. Call `ep.touch(handle)` to update LRU recency
8. Return result

**Entry Creation**:
1. Validate `size > 0`
2. `get_pool_id()` — lazily create eviction pool on first use
3. Lock `Mutex<Inner>`, check for duplicate key
4. `ep.track(pool_id, key)` — register in eviction policy, get handle
5. Insert `DispatchEntry` with `write_ref=1`, `Location::MemoryTier`
6. Caller holds exclusive write access until `release_write`

**State Transition (write-through eviction)**:
1. `convert_to_storage(key, offset)` — sets `ssd_offset` on MemoryTier entry, decrements `read_ref`
2. `convert_memory_tier_to_block(key)` — transitions entry from `MemoryTier{ssd_offset: Some(off)}` to `BlockDevice{offset: off}`
3. `is_evictable(key)` — predicate: MemoryTier + ssd_offset present + zero refs
4. `remove(key)` — deletes entry and removes from eviction policy

**Recovery**:
1. `initialize()` — iterate `IExtentManager.for_each_extent()`
2. For each persisted extent: create `BlockDevice` entry with zero refs, track in eviction policy
3. Gracefully handles unbound extent manager (starts empty)

### Key Design Decisions

1. **Single Mutex + Condvar**: All entries protected by one `Mutex<Inner>`. Simplifies correctness but means all contention funnels through one lock. The `wait_for()` helper loops on the condvar with a deadline, checking a per-key predicate each wakeup.

2. **Per-key reference counting (not per-entry lock)**: Each entry has independent `read_ref`/`write_ref` counters. Writers block all readers on the same key; readers on different keys proceed concurrently (they just share the HashMap mutex briefly).

3. **Lazy eviction pool creation**: The `PoolId` is created on first use via `get_pool_id()` and cached in `Mutex<Option<PoolId>>`. This avoids requiring the eviction policy to be bound before component construction.

4. **Unsafe Send+Sync for Location**: The `Location::MemoryTier` variant holds `*mut u8`. Safety is justified by the fact that all pointer access is serialized through the dispatch map's mutex and the memory-tier pool is thread-safe.

5. **Two-phase write-through**: Entries don't jump directly from MemoryTier to BlockDevice. First `convert_to_storage` records the SSD offset (data written but DRAM still valid), then `convert_memory_tier_to_block` finalizes the transition (DRAM can be freed).

6. **Timeout-based deadlock avoidance**: All blocking operations cap at 2 seconds (`DEFAULT_TIMEOUT`). Callers receive `Err(Timeout)` rather than deadlocking.

7. **Condvar notify_all on every mutation**: Every reference release, downgrade, and state conversion calls `condvar.notify_all()` to wake all waiters. Spurious wakeups are handled by re-checking the predicate.

## Dependencies

| Crate | Role | Path |
|-------|------|------|
| `component-framework` | `define_component!` macro, receptacle wiring | `../../component-framework/crates/component-framework` |
| `component-core` | `IUnknown`, `query_interface!` | `../../component-framework/crates/component-core` |
| `component-macros` | `define_interface!`, `define_component!` proc macros | `../../component-framework/crates/component-macros` |
| `interfaces` (feature: `spdk`) | `IDispatchMap`, `ILogger`, `IExtentManager`, `IEvictionPolicy` traits | `../../interfaces` |
| `eviction-policy-lru` (dev) | Concrete LRU for tests and benchmarks | `../../eviction-policy-lru` |
| `criterion 0.5` (dev) | Benchmark harness | crates.io |

## Testing

### Unit Tests (`src/lib.rs`)
- Reference counting: take/release read/write, underflow detection, overflow detection
- Downgrade: happy path, no-write-ref error
- Lookup: not exist, memory-tier, block-device
- State transitions: convert_to_storage, convert_memory_tier_to_block (happy + error cases)
- Removal: happy path, active references, key not found
- Eviction ordering: empty map, fewer than N, creation order, lookup updates timestamp
- Memory-tier entry creation: happy path, duplicates

### Integration Tests (`tests/integration.rs`)
- Multi-threaded concurrent access (4 readers)
- Writer blocks until readers release
- Writer timeout with active readers
- Lookup blocks on active writer
- Writer blocks another writer
- Sequential writers succeed
- Downgrade unblocks pending readers / still blocks writers
- Independent keys do not interfere
- Remove blocked by active read/write refs
- Concurrent readers and writer on different keys
- Recovery with mock `IExtentManager` (populated and empty)

### Benchmarks (`benches/dispatch_map_benchmark.rs`)
- `lookup_no_contention` — sub-microsecond target
- `take_release_read` — sub-microsecond target
- `take_release_write` — sub-microsecond target
- `oldest_keys_1000_entries_top10` / `top100` — LRU query latency
- `entry_size_check` — asserts `DispatchEntry` <= 56 bytes

### Formal Verification
10 properties verified with Creusot (P1-P10): read/write underflow, write binary, downgrade requires write, downgrade conservation, remove zero-refs, create no-duplicates, size nonzero, lookup increments read, convert requires ssd-offset.

## Future Considerations

1. **Sharded HashMap**: Replace single `Mutex<Inner>` with a sharded/concurrent map (e.g., `dashmap` or manual sharding) to reduce contention under high core counts.

2. **Per-entry condvar or futex**: Move from a global condvar (which wakes all waiters on any mutation) to per-key wait structures for reduced spurious wakeups.

3. **Capacity limits**: Add a maximum entry count with back-pressure or automatic eviction triggering when the map approaches capacity.

4. **Metrics/telemetry**: Export contention statistics (timeout count, average wait time, entry count) for operational visibility.

5. **Configurable timeout**: Allow the 2-second default timeout to be set per-operation or via component configuration rather than a compile-time constant.

6. **Batch operations**: Add `lookup_batch` / `release_batch` to amortize mutex acquisition over multiple keys for bulk I/O paths.
