# extent-manager-v2

An extent manager for AI-native storage that maps logical extent keys to
physical disk locations. It manages space allocation on a data device and
crash-consistent checkpointing on a dedicated metadata device.

## Overview

`ExtentManagerV2` implements the `IExtentManager` trait from the
`interfaces` crate. It provides:

- **Two-phase extent allocation** -- reserve space, write data, then atomically
  publish (or abort) the mapping
- **Region-sharded concurrency** -- keys are partitioned across N independent
  regions (power-of-two count), each with its own lock, buddy allocator,
  slab allocator, and per-slab key vectors
- **Crash-consistent checkpointing** -- extent metadata is persisted as a
  dual-copy contiguous blob on a dedicated metadata device, with CRC32 protection
  on both superblock and checkpoint regions
- **Checkpoint coalescing** -- concurrent checkpoint requests are coalesced so
  at most two I/O rounds execute instead of N

## API

The component implements `IExtentManager`:

```rust
// One-time setup
fn format(&self, params: FormatParams) -> Result<(), ExtentManagerError>;
fn initialize(&self) -> Result<(), ExtentManagerError>;

// Extent lifecycle
fn reserve_extent(&self, key: ExtentKey, size: u32) -> Result<WriteHandle, ExtentManagerError>;
fn remove_extent(&self, offset: u64) -> Result<(), ExtentManagerError>;

// Enumeration
fn get_extents(&self) -> Vec<Extent>;
fn for_each_extent(&self, cb: &mut dyn FnMut(&Extent));

// Persistence
fn checkpoint(&self) -> Result<(), ExtentManagerError>;
```

`WriteHandle` (returned by `reserve_extent`) is a RAII type:
- `.publish()` -- commit the mapping; returns an `Extent` with the assigned disk offset
- `.abort()` -- release the slot without publishing (drop also aborts)

Publishing with key `u64::MAX` (`FREE_KEY`) silently releases the slot and returns `Ok`
without making any mapping visible.

### Key types

| Type | Description |
|------|-------------|
| `ExtentKey` | `u64` -- caller-chosen logical identifier (`u64::MAX` is the free-slot sentinel) |
| `Extent` | `{ key, offset, size }` -- a published mapping from key to disk location |
| `WriteHandle` | RAII handle from `reserve_extent`; `.publish()` or `.abort()` / drop |
| `FormatParams` | `{ data_disk_size, sector_size, slab_size, max_extent_size, region_count, metadata_alignment, metadata_disk_ns_id, instance_id }` |

### Lifecycle

1. **Format** (first use): call `format(params)` to write the superblock and
   an initial checkpoint to the metadata device.
2. **Initialize** (subsequent boots): call `initialize()` to recover the slab
   state from the most recent valid checkpoint on disk.
3. **Reserve / publish / remove**: use `reserve_extent` to get a `WriteHandle`
   with a disk offset, write your data to that offset, then call `publish()` to
   make the mapping visible. Call `remove_extent(extent.offset)` when done.
4. **Checkpoint**: call `checkpoint()` periodically (or rely on the background
   checkpoint thread) to persist the current slab state to disk.

## How it works

### Disk layout

```
Metadata Device:
┌──────────┬──────────┬──────────────────┬──────────────────┐
│Superblock│ Padding  │ Checkpoint Copy 0│ Checkpoint Copy 1│
│  4 KiB   │ (align)  │ checkpoint_      │ checkpoint_      │
│          │          │ region_size      │ region_size      │
└──────────┴──────────┴──────────────────┴──────────────────┘

Data Device:
┌─────────────────────────────────────────────────────────────┐
│ Region 0 (buddy)│ Region 1 (buddy)│ ... │ Region N (buddy) │
│ slabs + extents │ slabs + extents │     │ slabs + extents  │
└─────────────────────────────────────────────────────────────┘
```

### Space allocation: buddy + slab

Each region has a **buddy allocator** that manages coarse-grained allocation of
slab-sized chunks (default 1 MiB). When an extent is requested, the slab layer
finds (or creates) a slab whose element size matches the block-aligned request
size, then allocates a slot from that slab's bitmap. A **size-class manager**
indexes slabs by element size for fast lookup.

Slabs are stored in a `BTreeMap<u64, Slab>` keyed by `start_offset`. With slab
sizes on the order of 1 GiB and disks up to 10 TB, a region can hold thousands
of slabs; the BTreeMap gives O(log n) offset-based lookup for `remove_extent`.

Each `Slab` carries a dense `Vec<u64>` of keys (one per slot). `FREE_KEY =
u64::MAX` marks unoccupied slots. This vector is the sole persistent mapping --
no separate extent index is maintained.

### Concurrency

Keys are sharded to regions by `key & (region_count - 1)`. Each region is
protected by a `parking_lot::RwLock`. Hot-path operations (`reserve_extent`,
`remove_extent`) only touch the target region's lock -- no global locks are
acquired.

Checkpoint coalescing uses a `Condvar`-based version scheme: if a checkpoint
is already in progress, arriving callers note they need the *next* completion
and wait, so at most two actual checkpoints execute regardless of how many
threads request one.

### Checkpoint format

A checkpoint is a single contiguous blob written to one of two fixed-size
regions on the metadata device (dual-copy rotation). The blob has a
16-byte header:

```
seq(8) | payload_len(4) | crc32(4) | payload...
```

The payload encodes the slab table for all regions:

```
u32 region_count
for each region:
    u32 num_slabs
    for each slab:
        u64 start_offset | u64 slab_size | u32 element_size | u32 num_slots
        [num_slots × u64] keys   (FREE_KEY = u64::MAX for unoccupied)
```

The superblock stores the active copy index and current checkpoint sequence
number. Since the superblock write is a single sector, it is atomic; the
inactive copy is always a valid fallback.

### Recovery

On `initialize()`, the recovery module:

1. Reads and validates the superblock (magic `CERTUSV5` + CRC32)
2. Reads the active checkpoint copy, verifying seq + CRC32
3. Falls back to the inactive copy if the active one fails
4. For each slab descriptor, reconstructs the allocation bitmap by scanning
   the key vector (slots where `key != FREE_KEY` are allocated)
5. Rebuilds the buddy allocator allocation state from the slab set
6. Rebuilds size-class managers for fast element-size lookup

## Build

```bash
cargo build -p extent-manager-v2
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
    data_disk_size: 1024 * 1024 * 1024,
    sector_size: 4096,
    slab_size: 1024 * 1024,
    max_extent_size: 65536,
    region_count: 4,
    metadata_alignment: 1048576,
    metadata_disk_ns_id: 1,
    instance_id: None,
}).unwrap();

let handle = component.reserve_extent(42, 4096).unwrap();
let extent = handle.publish().unwrap();
// extent.offset is the assigned disk byte offset
component.remove_extent(extent.offset).unwrap();
```

### Test Suites

| File | Coverage |
|------|----------|
| `tests/lifecycle.rs` | Reserve, publish, remove, abort, get_extents, for_each_extent, FREE_KEY discard |
| `tests/checkpoint.rs` | Checkpoint persistence, recovery after reboot, dual-copy rotation, fault injection |
| `tests/concurrent.rs` | Multi-threaded reserve/publish/remove, concurrent checkpoints |
| `tests/edge_cases.rs` | Boundary conditions, size classes, large extents |

## Benchmarks

Criterion-based benchmarks using `MockBlockDevice`:

```bash
cargo bench -p extent-manager-v2
```

Suites: `reserve_publish`, `enumerate`, `remove`, `checkpoint`.

## Component Framework

`ExtentManagerV2` is built with the `define_component!` macro from
`component-macros`. This provides receptacle-based dependency injection:

### Interfaces

| Interface | Role | Description |
|-----------|------|-------------|
| `IExtentManager` | Provided | Two-phase extent allocation, checkpointing, recovery |
| `IBlockDevice` (metadata) | Receptacle | Metadata NVMe device |
| `ILogger` | Receptacle | Structured logging |

## Source Layout

```
src/
  lib.rs            ExtentManagerV2 definition, IExtentManager impl
  bitmap.rs         Slab bitmap for slot-level allocation tracking
  block_io.rs       BlockDeviceClient wrapper (read/write at block granularity)
  buddy.rs          BuddyAllocator for coarse-grained slab allocation
  checkpoint.rs     Checkpoint write/read (dual-copy contiguous regions)
  error.rs          Error constructors
  recovery.rs       Checkpoint recovery: CRC validation, fallback, slab rebuild
  region.rs         RegionState (BTreeMap<u64,Slab>), SharedState
  slab.rs           Slab allocator with embedded key vectors; SizeClassManager
  superblock.rs     Superblock serialization (CERTUSV5 magic, CRC, copy pointers)
  test_support.rs   MockBlockDevice, FaultConfig, test helpers (feature = "testing")
tests/
  lifecycle.rs      Extent CRUD and lifecycle tests
  checkpoint.rs     Checkpoint persistence and recovery tests
  concurrent.rs     Multi-threaded concurrency tests
  edge_cases.rs     Boundary condition and error handling tests
benches/
  benchmarks.rs     Criterion benchmarks
```

## CI Gate

All must pass before merge:

```bash
cargo fmt -p extent-manager-v2 --check && \
cargo clippy -p extent-manager-v2 -- -D warnings && \
cargo test -p extent-manager-v2 && \
cargo doc -p extent-manager-v2 --no-deps && \
cargo bench -p extent-manager-v2 --no-run
```
