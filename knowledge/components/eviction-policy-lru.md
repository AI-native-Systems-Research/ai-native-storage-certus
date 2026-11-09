# eviction-policy-lru

**Crate**: `eviction-policy-lru`
**Path**: `components/eviction-policy-lru/`
**Version**: 0.1.0

## Description

LRU eviction policy component. Manages multiple independent eviction pools, each backed by an intrinsic doubly-linked list. Provides O(1) track/touch/remove and O(1) identify-next-to-evict. Used by the memory-tier and dispatch-map components to determine which cache entries to evict when capacity is exhausted.

## Component Definition

```
EvictionPolicyLruComponent {
    version: "0.1.0",
    provides: [IEvictionPolicy],
    receptacles: {
        logger: ILogger,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IEvictionPolicy {
        fn create_pool(&self) -> PoolId;
        fn track(&self, pool: PoolId, key: CacheKey) -> Result<EvictionHandle, EvictionPolicyError>;
        fn touch(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError>;
        fn batch_touch(&self, handles: &[EvictionHandle]) -> Result<(), EvictionPolicyError>;
        fn remove(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError>;
        fn identify_next_to_evict(&self, pool: PoolId) -> Option<CacheKey>;
        fn get_eviction_candidates(&self, pool: PoolId, n: usize) -> Vec<CacheKey>;
        fn len(&self, pool: PoolId) -> usize;
        fn clear_pool(&self, pool: PoolId);
    }
}
```

## Verified Properties

None. No formal verification model exists for this component.

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging (gracefully skips if unbound) |

## Key Types

- `PoolId = u32` — identifier for an independent eviction tracking pool
- `EvictionHandle { pool_id: u32, index: u32 }` — opaque handle for O(1) touch/remove
- `EvictionPolicyError` — `InvalidPool`, `InvalidHandle`

## Key Design Decisions

- **Multi-pool**: Each consumer (memory-tier, dispatch-map) gets its own pool — no cross-contamination of eviction order.
- **Handle-based API**: `track()` returns an `EvictionHandle` that encodes pool + list position. Subsequent operations use the handle for O(1) access.
- **Thread-safe**: Per-pool `Mutex` granularity — operations on different pools don't contend.
