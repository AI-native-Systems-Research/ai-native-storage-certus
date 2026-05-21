# extent-manager (v2)

Crash-consistent fixed-size extent allocator for the Certus storage system. Maps logical extent keys to physical disk locations using a region-sharded buddy+slab allocation scheme with dual-copy checkpoint persistence.

## Summary

`ExtentManagerV2` implements the `IExtentManager` trait. It provides:

- Two-phase extent allocation (reserve, write data, then publish or abort)
- Region-sharded concurrency (power-of-two count, each with its own lock, buddy allocator, slab allocator, and per-slab key vectors)
- Crash-consistent checkpointing on a dedicated metadata device (dual-copy rotation with CRC32 protection)
- Checkpoint coalescing (at most two IO rounds regardless of concurrent callers)
- Background periodic checkpoint thread (configurable interval, default 5 minutes)
- Recovery from the most recent valid checkpoint on initialization

### Disk Layout

```
Metadata Device:
+----------+----------+------------------+------------------+
|Superblock| Padding  | Checkpoint Copy 0| Checkpoint Copy 1|
|  4 KiB   | (align)  |                  |                  |
+----------+----------+------------------+------------------+

Data Device:
+------------------+------------------+-----+------------------+
| Region 0 (buddy) | Region 1 (buddy) | ... | Region N (buddy) |
| slabs + extents  | slabs + extents  |     | slabs + extents  |
+------------------+------------------+-----+------------------+
```

### Interfaces

| Interface | Role | Description |
|-----------|------|-------------|
| `IExtentManager` | Provided | Two-phase extent allocation, checkpointing, recovery |
| `IBlockDevice` (metadata) | Receptacle | Metadata NVMe device for superblock and checkpoints |
| `ILogger` | Receptacle | Structured logging |

## Structure

```
src/
  lib.rs            ExtentManagerV2 definition, IExtentManager impl, checkpoint coalescing
  bitmap.rs         Slab bitmap for slot-level allocation tracking
  block_io.rs       BlockDeviceClient wrapper (read/write at block granularity)
  buddy.rs          BuddyAllocator for coarse-grained slab allocation
  checkpoint.rs     Checkpoint write/read (dual-copy contiguous regions)
  error.rs          Error constructors
  recovery.rs       Checkpoint recovery: CRC validation, fallback, slab rebuild
  region.rs         RegionState (BTreeMap<u64,Slab>), SharedState
  slab.rs           Slab allocator with embedded key vectors; SizeClassManager
  superblock.rs     Superblock serialization (CERTUSV5 magic, CRC, copy pointers)
  write_handle.rs   WriteHandle RAII type (publish/abort)
  test_support.rs   MockBlockDevice, FaultConfig, test helpers (feature = "testing")
tests/
  lifecycle.rs      Extent CRUD and lifecycle tests
  checkpoint.rs     Checkpoint persistence and recovery tests
  concurrent.rs     Multi-threaded concurrency tests
  edge_cases.rs     Boundary condition and error handling tests
benches/
  benchmarks.rs     Criterion benchmarks (reserve_publish, enumerate, remove, checkpoint)
```

## Build and Test

This crate is excluded from the workspace `default-members` and must be built explicitly.

### Build

```bash
cargo build -p extent-manager-v2
```

### Features

| Feature | Description |
|---------|-------------|
| `spdk` (default) | Enable SPDK interface dependencies |
| `testing` | Expose `test_support` module with MockBlockDevice and FaultConfig |
| `volatile_write_cache` | Enable NVMe flush after checkpoint writes (for drives with volatile write cache) |

### Test

Tests use an in-memory `MockBlockDevice` and heap-based DMA allocation (via the `testing` feature).

```bash
cargo test -p extent-manager-v2
```

### Benchmarks

```bash
cargo bench -p extent-manager-v2
```

### Lint

```bash
cargo fmt -p extent-manager-v2 --check
cargo clippy -p extent-manager-v2 -- -D warnings
cargo doc -p extent-manager-v2 --no-deps
```
