# dispatch-map (v0)

Thread-safe dispatch map component for the Certus storage system. Maps extent keys to their current location with readers-writer reference counting for concurrent access.

## Summary

`DispatchMapComponentV0` implements the `IDispatchMap` interface. It tracks where extent data currently resides -- in a DMA staging buffer, a DRAM memory-tier pool, or at a block-device offset -- and provides per-entry readers-writer locking with timeout-based contention handling.

Key capabilities:
- Three-state entry locations: Staging (DMA buffer), MemoryTier (raw pointer + optional SSD offset), BlockDevice (disk offset)
- Per-entry read/write reference counting with timeout-based blocking
- Atomic downgrade from write to read reference (no unprotected window)
- LRU-style eviction support via TSC-timestamped entries and `oldest_keys(n)`
- Recovery from extent manager on initialization (rebuilds map from persisted extents)

### Entry Lifecycle

```
create_staging(key, size)          convert_to_storage(key, offset)
        |                                     |
        v                                     v
   +----------+                        +-----------------+
   | Staging  | ---------------------->| Block Device    |
   | (DMA buf)|                        | (offset)        |
   +----------+                        +-----------------+
        |                                     |
        +------------ remove(key) <-----------+

create_memory_tier_entry(key, ptr, size)
        |
        v
   +-------------+   convert_to_storage   +-------------+   convert_memory_tier_to_block   +--------------+
   | MemoryTier  | ----(sets ssd_offset)-->| MemoryTier  | -------------------------------->| Block Device |
   | (ptr, size) |                         | (ptr+offset)|                                  | (offset)     |
   +-------------+                         +-------------+                                  +--------------+
```

### Interfaces

| Interface | Role | Description |
|-----------|------|-------------|
| `IDispatchMap` | Provided | Extent key lookup, staging, storage commit, reference counting |
| `ILogger` | Receptacle | Info, debug, and error logging via dependency injection |
| `IExtentManager` | Receptacle | Extent iteration for recovery on initialization |

## Structure

```
src/
  lib.rs          Component definition (DispatchMapComponentV0), IDispatchMap impl
  entry.rs        DispatchEntry struct, Location enum (Staging/BlockDevice/MemoryTier), rdtsc
  state.rs        DispatchMapState — inner HashMap + Condvar-based wait_for helper
tests/
  integration.rs  Integration tests
benches/
  dispatch_map_benchmark.rs  Criterion benchmarks
```

## Build and Test

This crate requires SPDK interface dependencies and is not a default workspace member.

### Build

```bash
cargo build -p dispatch-map
```

### Test

```bash
cargo test -p dispatch-map
```

### Benchmarks

```bash
cargo bench -p dispatch-map
```

### Lint

```bash
cargo fmt -p dispatch-map --check
cargo clippy -p dispatch-map -- -D warnings
cargo doc -p dispatch-map --no-deps
```
