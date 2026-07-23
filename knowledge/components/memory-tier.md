# memory-tier

**Crate**: `memory-tier`
**Path**: `components/memory-tier/`
**Version**: 0.3.0
**Features**: `spdk` (SPDK allocation path), `telemetry` (contention/eviction counters)

## Description

DRAM memory-tier pool for caching data between GPU and SSD. Allocates a large contiguous region (via hugepages or SPDK DMA memory) and sub-allocates fixed-size slots keyed by `CacheKey`. Supports NUMA-aware placement via `mbind(MPOL_BIND)`. Delegates LRU ordering to an `IEvictionPolicy` receptacle for eviction decisions.

The pool is sharded into 16 independent sub-pools for reduced lock contention. Each shard holds its own allocation bitmap and slot map. Key-to-shard mapping is deterministic (key % 16).

## Component Definition

```
MemoryTierComponent {
    version: "0.2.0",
    provides: [IMemoryTier],
    receptacles: {
        logger: ILogger,
        eviction_policy: IEvictionPolicy,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IMemoryTier {
        fn initialize(&self, pool_size: usize, numa_node: Option<i32>) -> Result<(), MemoryTierError>;
        fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError>;
        fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)>;
        fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)>;
        fn evict_lru(&self) -> Option<CacheKey>;
        fn evict_lru_for_key(&self, key: CacheKey) -> Option<CacheKey>;
        fn oldest_keys(&self, n: usize) -> Vec<CacheKey>;
        fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError>;
        fn touch(&self, key: CacheKey);
        fn batch_touch(&self, keys: &[CacheKey]);
        fn contains(&self, key: CacheKey) -> bool;
        fn capacity(&self) -> usize;
        fn used(&self) -> usize;
        fn pool_info(&self) -> Option<(*mut u8, usize)>;
        fn is_dma_capable(&self) -> bool;
        fn clear(&self) -> Result<usize, MemoryTierError>;
        fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot;
    }
}
```

## Verified Properties

The following invariants are formally proved with Creusot (see `components/memory-tier/verif/`):

| ID | Name | Description |
|----|------|-------------|
| P1 | size-nonzero | `insert` rejects size == 0 |
| P2 | init-guard | insert/remove/get fail when pool not initialized |
| P3 | no-duplicates | `insert` rejects key that already has a slot |
| P4 | shard-bounded | `shard_for_key` always returns index < 16 |
| P5 | shard-deterministic | same key always maps to same shard |
| P6 | capacity-accounting | insert increases used by size; remove decreases by size |
| P7 | used-within-capacity | `used()` never exceeds `capacity()` |
| P8 | pool-full | `insert` returns PoolFull when used + size > capacity |
| P9 | remove-key-not-found | `remove` on absent key returns KeyNotFound |
| P10 | evict-round-robin | `evict_lru` cycles through all 16 shards |

Total: 10 properties, 21 verification conditions discharged by SMT solvers.

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging |
| `eviction_policy` | `IEvictionPolicy` | Yes | LRU ordering for eviction decisions |

## Key Types

- `MemoryTierError` — `PoolFull`, `KeyNotFound`, `AlreadyExists`, `AllocationFailed`, `InvalidSize`, `NotEvictable`, `NotInitialized`
