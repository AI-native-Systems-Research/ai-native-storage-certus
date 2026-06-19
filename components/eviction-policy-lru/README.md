# eviction-policy-lru

## Summary

LRU eviction policy component for the Certus storage system. Provides a pluggable eviction strategy that tracks cache entries across multiple independent pools and identifies the least-recently-used entry for eviction when capacity is exhausted.

`EvictionPolicyLruComponent` implements the `IEvictionPolicy` interface and declares a receptacle for `ILogger`. It supports multiple isolated pools (e.g., one per memory-tier or dispatch-map instance), O(1) touch/remove operations via a doubly-linked list, and thread-safe access via per-pool mutexes.

## Architecture

### Data Structure

Each pool contains an intrinsic doubly-linked LRU list (`LruList`) where:
- New entries are inserted at the tail (most recently used)
- `touch()` moves an entry to the tail
- `pop_oldest()` removes from the head (least recently used)
- `peek_oldest(n)` returns the N oldest keys without removal

### Pool Isolation

Pools are independent — entries in one pool are not affected by operations on another. This enables the memory-tier and dispatch-map to each maintain their own eviction order without interference.

## Interface

| Method | Description |
|--------|-------------|
| `create_pool() -> PoolId` | Create a new isolated eviction pool |
| `track(pool, key) -> EvictionHandle` | Start tracking a key in a pool |
| `touch(handle)` | Move entry to most-recently-used position |
| `remove(handle)` | Stop tracking an entry |
| `pop_oldest(pool) -> Option<CacheKey>` | Remove and return the LRU entry |
| `peek_oldest(pool, n) -> Vec<CacheKey>` | View the N oldest entries without removal |
| `len(pool) -> usize` | Number of tracked entries in a pool |
| `clear_pool(pool)` | Remove all entries from a pool |

## Receptacles

| Name | Interface | Purpose |
|------|-----------|---------|
| `logger` | `ILogger` | Diagnostic logging |

## Usage

```rust
use component_core::query_interface;
use interfaces::{IEvictionPolicy, CacheKey};

let comp = EvictionPolicyLruComponent::new_default();
let ep = query_interface!(comp, IEvictionPolicy).unwrap();

let pool = ep.create_pool();
let h1 = ep.track(pool, 42).unwrap();
let h2 = ep.track(pool, 99).unwrap();

ep.touch(h1).unwrap(); // 42 is now most-recently-used

let oldest = ep.pop_oldest(pool); // returns Some(99)
```

## Build & Test

```bash
cargo build -p eviction-policy-lru
cargo test -p eviction-policy-lru
cargo clippy -p eviction-policy-lru -- -D warnings
```
