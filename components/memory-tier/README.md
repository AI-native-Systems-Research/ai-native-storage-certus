# memory-tier

## Summary

The `memory-tier` component provides a DRAM-resident cache pool for the Certus storage system. Objects are inserted by a 64-bit key and tracked with LRU ordering so the least-recently-used entries can be evicted when space is needed. The pool is pre-allocated as a single contiguous region using `mmap` (preferring hugepage backing for TLB efficiency) and managed by a first-fit free-list allocator with 4 KiB alignment.

The component implements the `IMemoryTier` interface and declares an optional `ILogger` receptacle for operational logging. It is intended to sit between the block layer and higher-level extent/file logic, serving as a fast staging area for data that will be read or written to persistent storage.

## Architecture

### DRAM Cache Pool

On `initialize(pool_size)` the component maps a contiguous anonymous region via `mmap`. It first attempts `MAP_HUGETLB` for 2 MiB hugepage backing; if that fails (e.g., hugepages not configured), it falls back to regular 4 KiB pages. The default pool size is 256 MiB. All subsequent allocations come from within this region -- no further system calls are made on the data path.

### Eviction

Eviction ordering is delegated to a bound `IEvictionPolicy` component (currently `eviction-policy-lru`) via a receptacle — the memory-tier itself holds no eviction-order data structure and only tracks slot allocations. Every `get()` or `touch()` call updates the entry's position in the policy's ordering; `evict_next()` asks the policy for its next victim and frees the corresponding allocation.

### Allocator and DMA Integration

The `FreeList` allocator manages free regions using a `BTreeMap<offset, size>`. Allocations are 4 KiB-aligned, making them suitable for DMA transfers from NVMe devices. On deallocation the allocator coalesces adjacent free regions. Because the pool is contiguous and page-aligned, callers (e.g., the block-device layer) can pass returned pointers directly to DMA-capable I/O paths without additional alignment or pinning.

### Source Layout

```
src/
  lib.rs          Component definition, IMemoryTier impl, unit tests
  allocator.rs    First-fit free-list allocator (BTreeMap-based, 4 KiB aligned)
  lru.rs          Index-based doubly-linked list for O(1) LRU operations
```

## Build

```bash
cargo build -p memory-tier
```

Note: this crate depends on the `interfaces` crate with the `spdk` feature enabled.

## Test

```bash
cargo test -p memory-tier
```

Tests cover initialization, insert/get/remove, duplicate detection, pool-full behavior, LRU eviction ordering, touch promotion, capacity tracking, and data integrity (write-then-read verification).

## Benchmarks

No Criterion benchmarks are currently defined for this component.
