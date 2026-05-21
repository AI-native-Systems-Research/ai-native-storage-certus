# memory-tier

A DRAM memory pool with LRU eviction for caching in the Certus storage system. Implements the `IMemoryTier` interface as a component in the Certus framework.

## Summary

`MemoryTierComponentV0` provides a contiguous pre-allocated DRAM cache pool. Objects are inserted by key and tracked with LRU ordering for eviction. The pool is backed by `mmap` (with hugepage preference) and uses a first-fit free-list allocator with 4 KiB alignment.

Key features:
- Pre-allocated contiguous memory pool (default 256 MiB, configurable)
- First-fit free-list allocator with coalescing on deallocation
- O(1) LRU tracking via index-based doubly-linked list
- Hugepage-backed allocation (falls back to regular pages)
- Optional `ILogger` receptacle for operational logging

### IMemoryTier Interface

| Method | Description |
|--------|-------------|
| `initialize(pool_size)` | Allocate the memory pool via mmap |
| `insert(key, size)` | Allocate space and return a pointer to the slot |
| `get(key)` | Look up a key, promote in LRU, return pointer and size |
| `remove(key)` | Remove a key and free its allocation |
| `evict_lru()` | Evict the least recently used entry, return its key |
| `touch(key)` | Promote a key to most recently used without returning data |
| `contains(key)` | Check if a key exists in the pool |
| `capacity()` | Total pool capacity in bytes |
| `used()` | Currently allocated bytes |
| `pool_info()` | Return base pointer and size of the pool |

## Structure

```
src/
  lib.rs          MemoryTierComponentV0 definition, IMemoryTier implementation, unit tests
  allocator.rs    First-fit free-list allocator with 4 KiB alignment and coalescing
  lru.rs          Index-based doubly-linked list for O(1) LRU operations
```

## Build & Test

### Build

```bash
cargo build -p memory-tier
```

### Test

```bash
cargo test -p memory-tier
```

Tests cover initialization, insert/get/remove, duplicate detection, pool-full behavior, LRU eviction ordering, touch promotion, capacity tracking, and data integrity.
