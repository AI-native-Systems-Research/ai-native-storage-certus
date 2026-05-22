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
├── slabs: BTreeMap<u64, Slab>           -- key=start_offset; key vectors embedded in each Slab
├── size_classes: SizeClassManager       -- element_size -> [start_offsets]
├── buddy: BuddyAllocator               -- coarse allocation on data device
├── dirty: bool                          -- checkpoint skip optimization
├── pending_frees: Vec<(u64, usize)>    -- deferred slot frees (slab_start, slot_idx)
└── format_params: FormatParams
```

Each `Slab` embeds a dense `Vec<u64>` of keys (one per slot). `FREE_KEY = u64::MAX`
marks unoccupied slots. The allocation bitmap and the key vector are the two views
of the same data: the bitmap drives space reuse; the key vector drives enumeration
and persistence. On checkpoint the key vectors are serialized in place of a separate
index structure.

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
  1. Coalesce check: if checkpoint in progress, wait then wait for completion of next checkpoint, driving it if no other thread gets there first
  2. If no region is dirty, skip
  3. Serialize all regions (slab table with embedded key vectors per region)
  4. Determine inactive copy (1 - active_copy)
  5. Write contiguous blob to inactive checkpoint region on metadata device
  6. Update superblock: active_copy = inactive, bump checkpoint_seq
  7. Write superblock to metadata device LBA 0
  8. Clear dirty flags, flush pending_frees (mark slots as bitmap-free)
```

The serialized payload per region is:
```
u32 num_slabs
for each slab (BTreeMap order, i.e. by start_offset):
    u64 start_offset
    u64 slab_size
    u32 element_size
    u32 num_slots
    [num_slots × u64] keys   (FREE_KEY = u64::MAX for unoccupied)
```

### Recovery Flow

```
initialize()
  1. Read superblock at LBA 0 of metadata device, validate magic + CRC
  2. Read active checkpoint region, verify seq + CRC
  3. If active copy fails: read inactive copy as fallback
  4. Deserialize region data (slab tables with key vectors)
  5. Query data device size, set up buddy allocators per region
  6. For each slab descriptor: create Slab; for each slot i where keys[i] != FREE_KEY,
     call mark_slot_allocated(i) and set slab.keys[i] = keys[i];
     mark the corresponding buddy range as allocated
  7. Rebuild size class managers from the reconstructed slab set
  8. Insert slabs into RegionState BTreeMap
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

8. **Per-slab key vectors replace the extent index**: Each `Slab`
   carries a dense `Vec<u64>` of keys (one per slot) rather than a
   per-region `HashMap<ExtentKey, Extent>`. This eliminates a
   redundant data structure: the slab already tracks which slots are
   allocated; the key vector adds only the key assignment. Serializing
   the key vectors to disk replaces the old serialized index.

9. **FREE_KEY sentinel (u64::MAX)**: An unoccupied slot holds
   `u64::MAX`. Publishing with this key silently reclaims the slot and
   returns `Ok`, allowing callers to discard an extent without a
   separate abort path. The value is chosen to be unreachable by
   any reasonable key space.

10. **BTreeMap for slab storage**: Slabs are stored in a
    `BTreeMap<u64, Slab>` keyed by `start_offset`. With 1 GiB slabs
    on a 10 TB disk, a single region can hold thousands of slabs;
    O(log n) `range(..=offset).next_back()` lookup for
    `remove_extent(offset)` is necessary. `BTreeMap` also eliminates
    the `swap_remove`/index-fixup complexity that a `Vec` would
    require when deleting slabs.

11. **Offset-based remove**: `remove_extent(offset: u64)` replaces
    the old `remove_extent(key)`. The region for the offset is
    determined by uniform partition (`offset / region_bytes`); the
    slab is found via BTreeMap range lookup. This removes the need for
    a reverse key→offset mapping and allows O(log n) removal without
    a full scan.

## Project Structure

### Source Code

```text
components/extent-manager/v2/
├── Cargo.toml
├── README.md
├── src/
│   ├── lib.rs            -- component definition, IExtentManager impl
│   ├── error.rs          -- ExtentManagerError factory functions
│   ├── superblock.rs     -- on-disk superblock serialize/deserialize (v5)
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
├── iextent_manager.rs    -- IExtentManager trait (reserve_extent, publish,
│                            abort, remove_extent(offset), get_extents,
│                            for_each_extent, checkpoint, format, initialize),
│                            FormatParams, Extent, ExtentKey, WriteHandle,
│                            ExtentManagerError (OffsetNotFound, OutOfSpace, …)
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

`create_test_component(metadata_disk_size)` creates a metadata mock
device, wires it, and injects a heap DMA allocator.

Test files: `tests/lifecycle.rs`, `tests/checkpoint.rs`,
`tests/concurrent.rs`, `tests/edge_cases.rs`.

Benchmarks: `benches/benchmarks.rs` (Criterion).

## Future Considerations

- Async I/O: current implementation uses synchronous block I/O via
  command/completion channels. An async variant may improve throughput.
- Incremental checkpointing: currently the entire slab table (with key
  vectors) is rewritten on each checkpoint. At 100M extents across
  thousands of slabs this could be expensive; a delta/WAL approach
  could reduce checkpoint I/O significantly.
- Checkpoint compression: the payload is uncompressed; at scale,
  compression could reduce checkpoint I/O.
- Multi-data-device support: currently scoped to one data device.
  Supporting multiple data devices would require region-to-device
  mapping.
