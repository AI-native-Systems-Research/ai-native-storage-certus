# eviction-policy-lru

**Crate**: `eviction-policy-lru`
**Path**: `components/eviction-policy-lru/`
**Version**: 0.1.0

## Description

LRU eviction policy component. Manages multiple independent eviction pools, each backed by an intrinsic doubly-linked list. Provides O(1) track/touch/remove and O(1) pop-oldest for eviction. Used by the memory-tier and dispatch-map components to determine which cache entries to evict when capacity is exhausted.

## Component Definition

```
EvictionPolicyLruComponent {
    version: "0.1.0",
    provides: [IEvictionPolicy],
    receptacles: { logger: ILogger },
    fields: { state: RwLock<EvictionState> },
}
```

## Interfaces Provided

| Interface | Methods |
|-----------|---------|
| `IEvictionPolicy` | `create_pool() -> PoolId` |
|                   | `track(pool, key) -> Result<EvictionHandle, _>` |
|                   | `touch(handle) -> Result<(), _>` |
|                   | `remove(handle) -> Result<(), _>` |
|                   | `pop_oldest(pool) -> Option<CacheKey>` |
|                   | `peek_oldest(pool, n) -> Vec<CacheKey>` |
|                   | `len(pool) -> usize` |
|                   | `clear_pool(pool)` |

## Receptacles

| Name | Interface | Required |
|------|-----------|----------|
| `logger` | `ILogger` | No (gracefully skips if unbound) |

## Key Design Decisions

- **Multi-pool**: Each consumer (memory-tier, dispatch-map) gets its own pool — no cross-contamination of eviction order.
- **Handle-based API**: `track()` returns an `EvictionHandle` that encodes pool + list position. Subsequent operations use the handle for O(1) access.
- **Thread-safe**: Per-pool `Mutex` granularity — operations on different pools don't contend.

## Wired By

- `certus-server-yaml` profiles: wired to `dispatch_map.eviction_policy` and `memory_tier.eviction_policy`
- `certus-server`: manual wiring in `main.rs`
