# extent-manager

Fixed-size storage extent allocator for NVMe SSDs. Part of the Certus project.

## Overview

The extent manager allocates, tracks, and persists named extents (size-classified storage regions) on block devices. It uses a slab-based pool allocator with CRC-32 integrity checks and 4KiB-atomic writes for data safety.

## Architecture

### Slab-Based Pool Allocator

The device is divided into a pool of fixed-size slabs. Slabs are allocated on demand when a new extent size class is requested and no existing slab has free slots.

Each slab contains:
- **Bitmap region** — one bit per slot, rounded up to 4KiB blocks
- **Record region** — one 4KiB block per slot, holding extent metadata with CRC-32

```
Pool layout on device:

|--- Slab 0 (size class A) ---|--- Slab 1 (size class B) ---|--- free ---|
| bitmap blocks | record blocks | bitmap blocks | record blocks |          |
```

Maximum 256 slabs per pool.

### Extent Records

Each extent record is a 4096-byte block containing:
- Key (`u64`, bytes 0–7)
- Size class (`u32`, bytes 8–11)
- LBA offset (`u64`, bytes 12–19)
- Record CRC-32 checksum at bytes 4092–4096

### Write Protocol

1. Write the extent record block (with CRC-32) atomically
2. Flip the bitmap bit and persist the bitmap block
3. Update the in-memory index

## Interfaces

| Interface | Role | Description |
|-----------|------|-------------|
| `IExtentManager` | Provided | Extent lifecycle, CRUD operations, and initialization |
| `IBlockDevice` | Receptacle | Underlying NVMe block device |
| `ILogger` | Receptacle | Structured logging |

### IExtentManager API

| Method | Description |
|--------|-------------|
| `set_dma_alloc(alloc)` | Set the DMA allocator for block device I/O |
| `initialize(total_size_bytes, slab_size_bytes)` | Initialize the pool with given capacity and slab size |
| `create_extent(key, extent_size)` | Allocate an extent, returns `Extent` |
| `remove_extent(key)` | Deallocate an extent by key |
| `lookup_extent(key)` | Find an extent by key, returns `Extent` |
| `get_extents()` | Return all allocated extents as `Vec<Extent>` |
| `for_each_extent(cb)` | Iterate extents without allocation via callback |

### Extent

Returned by `create_extent`, `lookup_extent`, and `get_extents`:

```rust
pub struct Extent {
    pub key: ExtentKey,
    pub size: u32,
    pub offset: u64,
}
```

### Lifecycle

```
new_default() → wire receptacles → set_dma_alloc() → initialize() → use CRUD methods
```

## Build

```bash
# Default build (with SPDK feature)
cargo build -p extent-manager

# Without SPDK (for testing only)
cargo build -p extent-manager --no-default-features
```

This crate is excluded from the workspace `default-members` and must be built explicitly.

## Test

```bash
cargo test -p extent-manager
```

All tests run without NVMe hardware using `MockBlockDevice`, an in-memory block device implementation with optional fault injection.

### Test Suites

| File | Coverage |
|------|----------|
| `tests/api_operations.rs` | Create, remove, lookup, get_extents, error cases |
| `tests/thread_safety.rs` | Concurrent access patterns |

## Benchmarks

Criterion-based benchmarks using `MockBlockDevice`:

```bash
cargo bench -p extent-manager

# Individual suites
cargo bench -p extent-manager --bench create_benchmark
cargo bench -p extent-manager --bench remove_benchmark
cargo bench -p extent-manager --bench lookup_benchmark
```

## Source Layout

```
src/
  lib.rs            ExtentManagerComponentV0 definition, IExtentManager impl
  metadata.rs       ExtentMetadata, OnDiskExtentRecord (4KiB block with CRC-32)
  bitmap.rs         AllocationBitmap (bit-per-slot, multi-block serialization)
  block_device.rs   BlockDeviceClient wrapper (read_block/write_block at 4KiB granularity)
  state.rs          PoolState, SlabDescriptor, ExtentManagerState (in-memory index)
  error.rs          Error constructors
  test_support.rs   MockBlockDevice, FaultConfig, test helpers (feature = "testing")
tests/
  api_operations.rs   Extent CRUD and error handling tests
  thread_safety.rs    Concurrent access tests
benches/
  create_benchmark.rs
  remove_benchmark.rs
  lookup_benchmark.rs
```
