# Implementation Plan: Extent Manager V2

**Branch**: `001-extent-manager-v2` | **Date**: 2026-04-23 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

ExtentManagerV2 is a crash-consistent extent-to-disk-location mapper
for AI-native storage. It uses a two-level allocator (buddy + slab),
region-based sharding for concurrency, and a dual-chain checkpoint
format for resilience. The component is built on the Certus component
framework using `define_component!` with receptacle-based dependency
injection.

## Technical Context

**Language/Version**: Rust (workspace edition)
**Primary Dependencies**:
- `component-core` / `component-macros` / `component-framework` -- Certus component model
- `interfaces` (with `spdk` feature) -- shared trait definitions
- `crc32fast` -- CRC32 checksums for superblock and checkpoint chunks
- `parking_lot` -- `RwLock` with downgrade support for checkpoint serialization

**Storage**: Single NVMe block device via IBlockDevice receptacle
**Testing**: `cargo test` with in-memory MockBlockDevice and heap DMA allocation
**Target Platform**: Linux (SPDK/VFIO), macOS for development (mock-only)
**Performance Goals**: ~100M extents on a 10 TB device with 128 KiB extents
**Constraints**: Sector-atomic writes assumed; checkpoint must be crash-consistent

## Architecture

### Component Structure

```
ExtentManagerV2 (define_component!)
├── Receptacles
│   ├── block_device: IBlockDevice
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
├── buddy: BuddyAllocator               -- coarse allocation
├── dirty: bool                          -- checkpoint skip optimization
└── format_params: FormatParams
```

### Space Allocation: Buddy + Slab

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
  4. Allocate chunk blocks from per-region buddy allocators
  5. Write linked chain of CRC32-protected chunks
  6. Rotate superblock: previous <- current, current <- new chain
  7. Write superblock atomically (single sector)
  8. Free old previous chain blocks back to buddy allocators
```

### Recovery Flow

```
initialize()
  1. Read superblock at LBA 0, validate magic + CRC
  2. Follow current_index_lba chain, verify per-chunk CRCs
  3. If current chain fails: follow previous_index_lba chain
  4. Deserialize region data (index + slab descriptors)
  5. Rebuild buddy allocators, mark slab blocks as allocated
  6. Mark individual extent slots as allocated in slab bitmaps
  7. Rebuild size class managers
```

### Key Design Decisions

1. **Region sharding by key hash**: Keys are hashes, so
   `key & (region_count - 1)` gives uniform distribution. Each region
   is independently locked, so N regions allows N concurrent writers.

2. **Buddy + slab two-level allocation**: Buddy handles coarse (slab-
   sized) allocation with O(log N) splits/merges. Slab handles fine-
   grained allocation with O(1) bitmap scan. This avoids both external
   and internal fragmentation.

3. **Checkpoint coalescing**: A Condvar-based version scheme ensures
   at most two checkpoint I/O operations execute, regardless of how
   many threads request one. This prevents thundering-herd I/O.

4. **Dual checkpoint chains**: The superblock stores both current and
   previous chain pointers. Since the superblock write is sector-
   atomic, both pointers are always consistent. The previous chain is
   a fallback if media errors corrupt the current chain.

5. **Two-phase reserve/publish**: The caller gets a disk offset from
   `reserve_extent`, writes data there, then calls `publish()`. This
   ensures the mapping is only visible after data is on disk. Drop-
   as-abort provides safety if the caller forgets to commit.

6. **Rover-based slab allocation**: The bitmap rover distributes
   allocations across slots, reducing hot-spotting on the first slots.

## Project Structure

### Source Code

```text
components/extent-manager/v2/
├── Cargo.toml
├── README.md
├── src/
│   ├── lib.rs            -- component definition, IExtentManager impl
│   ├── error.rs          -- ExtentManagerError factory functions
│   ├── superblock.rs     -- on-disk superblock serialize/deserialize
│   ├── checkpoint.rs     -- checkpoint write/read/deserialize
│   ├── recovery.rs       -- recover() from superblock + checkpoint
│   ├── region.rs         -- RegionState, SharedState
│   ├── buddy.rs          -- BuddyAllocator
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
- Multi-device support: currently scoped to a single block device.
