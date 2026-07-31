# Implementation Plan: LRU Eviction Policy

**Branch**: `001-lru-eviction-policy` | **Date**: 2026-06-19 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

A shared LRU eviction policy component providing O(1) track/touch/remove/pop operations via an index-based doubly-linked list. Supports multiple independent pools for use across memory-tier and dispatch-map components.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `component-framework` — Component model macros and lifecycle
- `component-core` — `query_interface!` for runtime interface discovery
- `interfaces` — `IEvictionPolicy` trait and associated types (`EvictionHandle`, `EvictionKey`, `PoolId`, `EvictionPolicyError`)

**Performance Goals**: O(1) for all single-entry operations. Per-pool locking to minimize contention.

## Architecture

### Component Layer

```
EvictionPolicyLruComponent
├── provides: IEvictionPolicy
├── receptacles: ILogger
└── fields:
    └── state: RwLock<EvictionState>
        └── pools: Vec<Mutex<Pool>>
            └── lru: LruList
```

### Data Structure: LruList

```
LruList
├── nodes: Vec<Node>       // index-addressed, avoids heap alloc per entry
├── head: Option<u32>      // front = least recently used
├── tail: Option<u32>      // back = most recently used
├── free: Vec<u32>         // recycled slot indices
└── len: usize             // active entry count

Node { key: u64, prev: Option<u32>, next: Option<u32>, active: bool }
```

### Concurrency Model

1. `RwLock<EvictionState>` protects the pool vector:
   - **Read lock**: all per-pool operations (track, touch, remove, pop, peek, len, clear)
   - **Write lock**: only `create_pool()` (appending to the pool vec)
2. Each pool has its own `Mutex<Pool>` — operations on different pools never contend.

### Operation Flow

| Operation | Lock | Complexity |
|-----------|------|-----------|
| `create_pool()` | write(state) | O(1) amortized |
| `track(pool, key)` | read(state) + lock(pool) | O(1) |
| `touch(handle)` | read(state) + lock(pool) | O(1) |
| `remove(handle)` | read(state) + lock(pool) | O(1) |
| `identify_next_to_evict(pool)` | read(state) + lock(pool) | O(1) |
| `get_eviction_candidates(pool, n)` | read(state) + lock(pool) | O(n) |
| `len(pool)` | read(state) + lock(pool) | O(1) |
| `clear_pool(pool)` | read(state) + lock(pool) | O(1) (drops Vec) |

### Key Design Decisions

1. **Index-based linked list over pointer-based**: Cache-friendly, no per-node heap allocation, and compatible with the `u32` index embedded in `EvictionHandle`.
2. **Free-list recycling**: Removed node slots are reused so long-lived pools with high churn don't accumulate dead memory.
3. **Idempotent operations on stale handles**: The `active` flag allows `remove` and `move_to_back` on already-removed nodes without panic — consumers can safely issue redundant removes.
4. **Per-pool Mutex (not RwLock)**: All pool operations are mutations (even `peek` needs traversal consistency); a Mutex is simpler and cheaper than a RwLock for short critical sections.
5. **RwLock at top level**: Allows all per-pool operations to proceed concurrently without blocking on pool creation.

## Project Structure

```text
components/eviction-policy-lru/
├── Cargo.toml
├── CLAUDE.md
├── src/
│   ├── lib.rs          # Component definition, IEvictionPolicy impl, integration tests
│   └── lru_list.rs     # Index-based doubly-linked list, unit tests
└── .specify/
    └── specs/001-lru-eviction-policy/
        ├── spec.md
        ├── plan.md
        └── tasks.md
```

## Dependencies (Consumer Graph)

```
eviction-policy-lru
├── dispatch-map (1 pool for key→extent mapping)
├── memory-tier (16 pools, one per NUMA/tier partition)
├── dispatcher (benchmark harness)
├── dispatcher-p2p (benchmark harness)
├── certus-connector (Python bindings)
├── certus-server (direct integration)
└── certus-server-yaml (YAML-driven profiles)
```

## Testing

- **Unit tests** (`src/lru_list.rs`): 12 tests covering push/pop, move_to_back, remove (head/middle/tail), free-list reuse, single-element edge case, len tracking, clear, idempotent remove/move, peek.
- **Integration tests** (`src/lib.rs`): 8 tests covering component-model wiring via `query_interface!`, pool independence, sequential pool IDs, FIFO ordering, touch reordering, remove invalidation, peek non-destructiveness, invalid pool errors, concurrent multi-thread stress.

## Future Considerations

- Sharded LRU (per-shard locking) for higher concurrency if profiling shows pool-mutex contention.
- Approximate LRU policies (e.g., Clock, SLRU) as alternative implementations of `IEvictionPolicy` for different workload characteristics.
- Capacity limits per pool with automatic eviction callbacks.
