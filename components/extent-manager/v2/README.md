# extent-manager-v2

A metadata manager for AI-native storage that maps logical extent keys to
physical disk locations. It manages space allocation, crash-consistent
checkpointing, and recovery for a single block device.

## Overview

`MetadataManager` implements the `IExtentManager` trait from the
`interfaces` crate. It provides:

- **Two-phase extent allocation** -- reserve space, write data, then atomically
  publish (or abort) the mapping
- **Region-sharded concurrency** -- keys are partitioned across N independent
  regions (power-of-two count), each with its own lock, index, buddy allocator,
  and slab allocator
- **Crash-consistent checkpointing** -- extent metadata is persisted as a
  linked chain of CRC-protected chunks, with dual-chain rotation so the
  previous checkpoint remains available if media errors corrupt the current one
- **Checkpoint coalescing** -- concurrent checkpoint requests are coalesced so
  at most two I/O rounds execute instead of N

## API

The component implements `IExtentManager`:

```rust
// One-time setup
fn set_dma_alloc(&self, alloc: DmaAllocFn);
fn format(&self, params: FormatParams) -> Result<(), ExtentManagerError>;
fn initialize(&self) -> Result<(), ExtentManagerError>;

// Extent lifecycle
fn reserve_extent(&self, key: ExtentKey, size: u32) -> Result<WriteHandle, ExtentManagerError>;
fn lookup_extent(&self, key: ExtentKey) -> Result<Extent, ExtentManagerError>;
fn remove_extent(&self, key: ExtentKey) -> Result<(), ExtentManagerError>;

// Enumeration
fn get_extents(&self) -> Vec<Extent>;
fn for_each_extent(&self, cb: &mut dyn FnMut(&Extent));

// Persistence
fn checkpoint(&self) -> Result<(), ExtentManagerError>;
```

### Key types

| Type | Description |
|------|-------------|
| `ExtentKey` | `u64` -- caller-chosen logical identifier |
| `Extent` | `{ key, offset, size }` -- a published mapping from key to disk location |
| `WriteHandle` | RAII handle from `reserve_extent`; call `.publish()` to commit or `.abort()` (or drop) to release |
| `FormatParams` | `{ sector_size, slab_size, max_element_size, metadata_block_size, region_count }` |

### Lifecycle

1. **Format** (first use): call `format(params)` to write a superblock and
   initialize region state.
2. **Initialize** (subsequent boots): call `initialize()` to recover the index
   from the most recent valid checkpoint on disk.
3. **Reserve / publish / remove**: use `reserve_extent` to get a `WriteHandle`
   with a disk offset, write your data to that offset, then call `publish()` to
   make it visible. Call `remove_extent` when done.
4. **Checkpoint**: call `checkpoint()` periodically (or rely on the background
   checkpoint thread) to persist the current index to disk.

## How it works

### Disk layout

The entire disk is managed by per-region buddy allocators. Each region owns a
contiguous byte range. There is no reserved superblock area -- the superblock
occupies slot 0 of region 0's metadata slab.

```
Region 0                Region 1               Region N-1
+---------+--------+    +----------+--------+   +----------+--------+
| meta    | user   |    | meta     | user   |   | meta     | user   |
| slab    | slabs  |    | slab     | slabs  |   | slab     | slabs  |
+---------+--------+    +----------+--------+   +----------+--------+
 ^                       ^                       ^
 slot 0 = superblock     buddy-managed           buddy-managed
```

### Space allocation: buddy + slab

Each region has a **buddy allocator** that manages coarse-grained allocation of
slab-sized chunks (default 1 MiB). When an extent is requested, the slab layer
finds (or creates) a slab whose element size matches the block-aligned request
size, then allocates a slot from that slab's bitmap. A **size-class manager**
indexes slabs by element size for fast lookup.

This two-level scheme avoids external fragmentation (buddy) while efficiently
packing same-size extents (slab bitmap).

### Concurrency

Keys are sharded to regions by `key & (region_count - 1)`. Each region is
protected by a `parking_lot::RwLock`. Hot-path operations (`reserve_extent`,
`lookup_extent`, `remove_extent`) only touch the target region's lock --
no global locks are acquired.

Checkpoint coalescing uses a `Condvar`-based version scheme: if a checkpoint
is already in progress, arriving callers note they need the *next* completion
and wait, so at most two actual checkpoints execute regardless of how many
threads request one.

### Checkpoint format

A checkpoint is a linked list of fixed-size chunks (each `metadata_block_size` bytes),
where each chunk has a CRC32-protected header:

```
magic(4) | seq(8) | prev_lba(8) | next_lba(8) | payload_len(4) | checksum(4) | payload...
```

The concatenated payload across all chunks encodes every region's extent
index and slab descriptors. The superblock at LBA 0 stores pointers to
both the current and previous checkpoint chains. Since the superblock
write is atomic (single block), both pointers are always consistent. The
previous chain serves as a fallback if media errors make the current
chain unreadable.

### Recovery

On `initialize()`, the recovery module:

1. Reads and validates the superblock (magic + CRC)
2. Follows the current checkpoint chain, verifying per-chunk CRCs
3. Falls back to the previous chain if media errors make the current one unreadable
4. Rebuilds each region's buddy allocator, slab state, and extent index
   from the recovered data

## Build

```bash
# Default build (with SPDK feature)
cargo build -p extent-manager-v2

# Without SPDK (for testing only)
cargo build -p extent-manager-v2 --no-default-features
```

This crate is excluded from the workspace `default-members` and must be built explicitly.

## Test

Tests use an in-memory `MockBlockDevice` and heap-based DMA allocation,
both provided by the `test_support` module (gated on the `testing` feature).

```bash
cargo test -p extent-manager-v2
```

The mock supports fault injection (`FaultConfig`) for testing write failures,
and `reboot_from(shared_state)` to simulate device reboots over the same
backing store.

```rust
use extent_manager_v2::test_support::create_test_component;
use interfaces::{FormatParams, IExtentManager};

let (component, _mock) = create_test_component(64 * 1024 * 1024);
component.format(FormatParams {
    sector_size: 4096,
    slab_size: 1024 * 1024,
    max_element_size: 65536,
    metadata_block_size: 131072,
    region_count: 4,
}).unwrap();

let handle = component.reserve_extent(42, 4096).unwrap();
let extent = handle.publish().unwrap();
assert_eq!(component.lookup_extent(42).unwrap().offset, extent.offset);
```

### Test Suites

| File | Coverage |
|------|----------|
| `tests/lifecycle.rs` | Reserve, publish, lookup, remove, abort, get_extents, for_each_extent |
| `tests/checkpoint.rs` | Checkpoint persistence, recovery after reboot, dual-chain rotation |
| `tests/concurrent.rs` | Multi-threaded reserve/publish/lookup, concurrent checkpoints |
| `tests/edge_cases.rs` | Key zero, duplicate keys, large extents, boundary conditions |

## Benchmarks

Criterion-based benchmarks using `MockBlockDevice`:

```bash
cargo bench -p extent-manager-v2
```

## Component Framework

`MetadataManager` is built with the `define_component!` macro from
`component-macros`. This provides receptacle-based dependency injection:
the `block_device` and `logger` receptacles are wired at assembly time,
decoupling the component from concrete implementations.

### Interfaces

| Interface | Role | Description |
|-----------|------|-------------|
| `IExtentManagerV2` | Provided | Two-phase extent allocation, checkpointing, recovery |
| `IBlockDevice` | Receptacle | Underlying NVMe block device |
| `ILogger` | Receptacle | Structured logging |

## Source Layout

```
src/
  lib.rs            MetadataManagerV2 definition, IExtentManagerV2 impl
  bitmap.rs         Slab bitmap for slot-level allocation tracking
  block_io.rs       BlockDeviceClient wrapper (read/write at block granularity)
  buddy.rs          BuddyAllocator for coarse-grained slab allocation
  checkpoint.rs     Checkpoint write: linked chain of CRC-protected chunks
  error.rs          Error constructors
  recovery.rs       Checkpoint recovery: chain traversal, fallback, index rebuild
  region.rs         RegionState, SharedState, SizeClassManager
  slab.rs           Slab allocator (element-size bitmap within buddy-allocated chunks)
  superblock.rs     Superblock serialization (magic, CRC, chain pointers)
  test_support.rs   MockBlockDevice, FaultConfig, test helpers (feature = "testing")
  write_handle.rs   WriteHandle RAII type (publish/abort semantics)
tests/
  lifecycle.rs      Extent CRUD and lifecycle tests
  checkpoint.rs     Checkpoint persistence and recovery tests
  concurrent.rs     Multi-threaded concurrency tests
  edge_cases.rs     Boundary condition and error handling tests
benches/
  benchmarks.rs     Criterion benchmarks (reserve/publish throughput)
```

## CI Gate

All must pass before merge:

```bash
cargo fmt -p extent-manager-v2 --check && cargo clippy -p extent-manager-v2 -- -D warnings && cargo test -p extent-manager-v2 && cargo doc -p extent-manager-v2 --no-deps && cargo bench -p extent-manager-v2 --no-run
```
