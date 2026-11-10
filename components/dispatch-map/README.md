# dispatch-map

## Summary

Thread-safe dispatch map component for the Certus storage system. Maps extent keys (`CacheKey`) to their current storage location -- an in-memory DMA staging buffer, a DRAM memory-tier pool, or a block-device offset -- with per-entry readers-writer reference counting for concurrent access.

`DispatchMapComponent` implements the `IDispatchMap` interface and declares receptacles for `ILogger` (diagnostics) and `IExtentManager` (recovery). It provides timeout-based blocking on contention, atomic write-to-read reference downgrade, and LRU-style eviction support via RDTSC timestamps and `oldest_keys(n)`.

## Architecture

### Data Structure

The map is a `HashMap<CacheKey, DispatchEntry>` protected by a `Mutex` with a `Condvar` for blocking waiters. Each `DispatchEntry` contains:

- **Location** -- one of three variants:
  - `Staging { buffer: Arc<DmaBuffer> }` -- data lives in a DMA-aligned staging buffer.
  - `BlockDevice { offset: u64 }` -- data has been committed to persistent storage.
  - `MemoryTier { pointer, size, ssd_offset }` -- data lives in the DRAM pool with an optional write-through SSD offset.
- **size_blocks** -- extent size in 4 KiB blocks.
- **read_ref / write_ref** -- per-entry reference counts (readers-writer, not shared/exclusive atomics).
- **tsc** -- RDTSC timestamp updated on creation, lookup, and touch; used to identify the coldest entries.

### Lookup Semantics

`lookup(key)` blocks (up to a configurable timeout, default 2 s) until the entry's write reference drops to zero, then increments the read reference and returns a `LookupResult` variant matching the current location. If the key does not exist, `LookupResult::NotExist` is returned immediately without blocking.

`take_write(key)` blocks until both read and write references are zero. `take_read(key)` blocks until the write reference is zero. These primitives enable exclusive staging writes followed by concurrent reads after `downgrade_reference` or `release_write`.

### Entry Lifecycle

```
create_staging(key, size)          convert_to_storage(key, offset)
        |                                     |
        v                                     v
   +----------+                        +-----------------+
   | Staging  | ---------------------->| Block Device    |
   | (DMA buf)|                        | (offset)        |
   +----------+                        +-----------------+

create_memory_tier_entry(key, ptr, size)
        |
        v                  convert_to_storage          convert_memory_tier_to_block
   +-------------+      (sets ssd_offset only)      +-------------+
   | MemoryTier  | -------------------------------->| Block Device |
   | (ptr, size) |                                  | (offset)     |
   +-------------+                                  +--------------+
```

### Recovery

`initialize()` walks persisted extents via the `IExtentManager` receptacle and rebuilds the in-memory map with `BlockDevice` locations. Staging buffers are not recovered.

## Build

This crate depends on SPDK interface features and is not a default workspace member. Build explicitly:

```bash
cargo build -p dispatch-map
```

## Test

```bash
cargo test -p dispatch-map
```

Unit tests cover staging creation, lookup across all location variants, reference counting (including overflow/underflow), timeout behavior, eviction ordering, and memory-tier lifecycle transitions.

## Benchmarks

Criterion-based benchmarks are in `benches/dispatch_map_benchmark.rs`. Run with:

```bash
cargo bench -p dispatch-map
```
