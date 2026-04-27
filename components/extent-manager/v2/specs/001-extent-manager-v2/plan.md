# Implementation Plan: Extent Manager V2

**Branch**: `001-extent-manager-v2` | **Date**: 2026-04-27 | **Spec**: [spec.md](spec.md)
**Context**: Updated for two-device architecture (separate data + metadata disks).

## Summary

ExtentManagerV2 is a crash-consistent extent-to-disk-location mapper
for AI-native storage. It uses a two-level allocator (buddy + slab) on
a dedicated data device, region-based sharding for concurrency, and a
dual-copy checkpoint format on a dedicated metadata device for
resilience. The component is built on the Certus component framework
using `define_component!` with receptacle-based dependency injection.

## Technical Context

**Language/Version**: Rust (workspace edition)
**Primary Dependencies**:
- `component-core` / `component-macros` / `component-framework` -- Certus component model
- `interfaces` (with `spdk` feature) -- shared trait definitions
- `crc32fast` -- CRC32 checksums for superblock and checkpoint regions
- `parking_lot` -- `RwLock` with downgrade support for checkpoint serialization

**Storage**: Two NVMe block devices via IBlockDevice receptacles
  - `block_device` — data device for user extents
  - `metadata_device` — metadata device for superblock + checkpoint regions
**Testing**: `cargo test` with in-memory MockBlockDevice and heap DMA allocation
**Target Platform**: Linux (SPDK/VFIO), macOS for development (mock-only)
**Performance Goals**: ~100M extents on a 10 TB data device with 128 KiB extents
**Constraints**: Sector-atomic writes assumed; checkpoint must be crash-consistent

## Architecture

### Component Structure

```
ExtentManagerV2 (define_component!)
├── Receptacles
│   ├── block_device: IBlockDevice     (data device)
│   ├── metadata_device: IBlockDevice  (metadata device)
│   └── logger: ILogger
├── State
│   ├── regions: RwLock<Vec<Arc<RwLock<RegionState>>>>
│   ├── shared: Mutex<SharedState>
│   └── checkpoint_coalesce: Mutex<CheckpointCoalesce> + Condvar
├── Background
│   ├── checkpoint_thread: JoinHandle
│   ├── checkpoint_interval_ms: AtomicU64 (default 5000)
│   └── shutdown: Arc<AtomicBool>
└── Provides: IExtentManager
```

### Per-Region State

```
RegionState
├── index: HashMap<ExtentKey, Extent>    -- the extent map
├── slabs: Vec<Slab>                     -- slab allocators
├── size_classes: SizeClassManager       -- element_size -> [slab indices]
├── buddy: BuddyAllocator               -- coarse allocation on data device
├── dirty: bool                          -- checkpoint skip optimization
├── pending_frees: Vec<(usize, usize)>  -- deferred slot frees
└── format_params: FormatParams
```

### Device Layout

```
Metadata Device:
┌──────────┬────────────┬──────────────────┬──────────────────┐
│Superblock│  Padding   │ Checkpoint Copy 0│ Checkpoint Copy 1│
│  4 KiB   │ (to       │ checkpoint_      │ checkpoint_      │
│          │ alignment) │ region_size      │ region_size      │
└──────────┴────────────┴──────────────────┴──────────────────┘

Data Device:
┌─────────────────────────────────────────────────────────────┐
│ Region 0 (buddy)│ Region 1 (buddy)│ ... │ Region N (buddy) │
│ slabs + extents │ slabs + extents │     │ slabs + extents  │
└─────────────────────────────────────────────────────────────┘
```

### Space Allocation: Buddy + Slab (data device only)

Two-level scheme avoids external fragmentation (buddy manages slab-
sized chunks) while efficiently packing same-size extents (slab
bitmap).

```
reserve_extent(key, size)
  1. Compute element_size = align_up(size, sector_size)
  2. region = key & (region_count - 1)
  3. Search size_classes for existing slab with matching element_size
  4. If found: alloc_slot() from slab bitmap (rover-based)
  5. If not: buddy.alloc(slab_size) -> new Slab -> alloc_slot()
  6. Return WriteHandle { key, offset, size, publish_fn, abort_fn }
```

### Checkpoint Flow

```
checkpoint()
  1. Coalesce check: if checkpoint in progress, wait for next completion
  2. If no region is dirty, skip
  3. Serialize all regions (index entries + slab descriptors)
  4. Determine inactive copy (1 - active_copy)
  5. Write contiguous blob to inactive checkpoint region on metadata device
  6. Update superblock: active_copy = inactive, bump checkpoint_seq
  7. Write superblock to metadata device LBA 0
  8. Clear dirty flags, flush pending frees
```

### Recovery Flow

```
initialize()
  1. Read superblock at LBA 0 of metadata device, validate magic + CRC
  2. Read active checkpoint region, verify seq + CRC
  3. If active copy fails: read inactive copy as fallback
  4. Deserialize region data (index + slab descriptors)
  5. Query data device size, set up buddy allocators per region
  6. Rebuild slab bitmaps from index entries
  7. Rebuild size class managers
```

### Key Design Decisions

1. **Separate metadata and data devices**: Metadata goes on a
   dedicated device with a simple contiguous layout. This decouples
   metadata I/O from data I/O and eliminates the need to allocate
   checkpoint storage from the data device's buddy allocator.

2. **Two contiguous checkpoint copies**: Instead of linked chunk
   chains allocated from buddy, each checkpoint is a single
   contiguous write to a fixed region. This simplifies both the
   write path (no chunk allocation) and the read path (no chain
   following).

3. **Region sharding by key hash**: Keys are hashes, so
   `key & (region_count - 1)` gives uniform distribution. Each region
   is independently locked, so N regions allows N concurrent writers.

4. **Buddy + slab two-level allocation**: Buddy handles coarse (slab-
   sized) allocation with O(log N) splits/merges. Slab handles fine-
   grained allocation with O(1) bitmap scan. This avoids both external
   and internal fragmentation.

5. **Checkpoint coalescing**: A Condvar-based version scheme ensures
   at most two checkpoint I/O operations execute, regardless of how
   many threads request one. This prevents thundering-herd I/O.

6. **Two-phase reserve/publish**: The caller gets a disk offset from
   `reserve_extent`, writes data there, then calls `publish()`. This
   ensures the mapping is only visible after data is on disk. Drop-
   as-abort provides safety if the caller forgets to commit.

7. **Deferred slot freeing**: Removed extents keep their disk slots
   allocated until after the next successful checkpoint, preventing
   a crash-after-reallocation corruption scenario.

## Project Structure

### Source Code

```text
components/extent-manager/v2/
├── Cargo.toml
├── README.md
├── src/
│   ├── lib.rs            -- component definition, IExtentManager impl
│   ├── error.rs          -- ExtentManagerError factory functions
│   ├── superblock.rs     -- on-disk superblock serialize/deserialize (v3)
│   ├── checkpoint.rs     -- checkpoint write/read (contiguous regions)
│   ├── recovery.rs       -- recover() from superblock + checkpoint
│   ├── region.rs         -- RegionState, SharedState
│   ├── buddy.rs          -- BuddyAllocator (data device)
│   ├── slab.rs           -- Slab, SizeClassManager
│   ├── bitmap.rs         -- AllocationBitmap
│   ├── block_io.rs       -- BlockDeviceClient (sync wrapper)
│   ├── write_handle.rs   -- (stub; WriteHandle defined in interfaces)
│   └── test_support.rs   -- MockBlockDevice, MockLogger, helpers
├── tests/
│   ├── lifecycle.rs      -- basic CRUD + enumeration
│   ├── checkpoint.rs     -- persistence + recovery + fallback
│   ├── concurrent.rs     -- multi-threaded correctness
│   └── edge_cases.rs     -- boundary conditions, size classes
└── benches/
    └── benchmarks.rs     -- Criterion benchmarks
```

### Interface Dependencies

```text
components/interfaces/src/
├── iextent_manager.rs    -- IExtentManager trait, FormatParams, Extent,
│                            ExtentKey, WriteHandle, ExtentManagerError
└── iblock_device.rs      -- IBlockDevice trait (receptacle type)
```

## Testing

Tests use `MockBlockDevice` (in-memory HashMap-backed block store) and
`heap_dma_alloc` (standard heap allocation pretending to be DMA). The
mock supports:
- `FaultConfig` for injecting write failures
- `reboot_from(shared_state)` for simulating device reboots over the
  same backing store
- `shared_state()` to extract the backing store for reboot simulation

`create_test_component(data_disk_size, metadata_disk_size)` creates
both mock devices and wires both receptacles.

Test files: `tests/lifecycle.rs`, `tests/checkpoint.rs`,
`tests/concurrent.rs`, `tests/edge_cases.rs`.

Benchmarks: `benches/benchmarks.rs` (Criterion).

## Future Considerations

- Async I/O: current implementation uses synchronous block I/O via
  command/completion channels. An async variant may improve throughput.
- Incremental checkpointing: currently the entire index is rewritten
  on each checkpoint. At 100M extents this could be expensive.
- Checkpoint compression: the payload is uncompressed; at scale,
  compression could reduce checkpoint I/O.
- Multi-data-device support: currently scoped to one data device.
  Supporting multiple data devices would require region-to-device
  mapping.
