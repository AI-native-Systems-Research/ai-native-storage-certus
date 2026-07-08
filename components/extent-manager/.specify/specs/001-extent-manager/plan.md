# Implementation Plan: Extent Manager

**Branch**: `001-extent-manager` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The Extent Manager is a crash-consistent, fixed-size extent allocator providing two-phase allocation (reserve/publish/abort) over NVMe block devices. It maps logical `u64` keys to physical disk offsets using region-sharded buddy+slab allocation. Durability is achieved through dual-copy alternating checkpoints with CRC32 integrity, enabling recovery from single-copy corruption. The component integrates into the Certus COM-style framework via `define_component!` with `IBlockDevice` and `ILogger` receptacles.

## Technical Context

- **Language**: Rust stable, edition 2021, MSRV 1.75
- **Component version**: 0.3.0
- **On-disk format version**: 6
- **Framework integration**: `define_component!` macro provides IUnknown, receptacle wiring, and version metadata
- **Concurrency model**: Region-sharded `parking_lot::RwLock` -- no global lock on the hot allocation path
- **I/O model**: Synchronous block I/O over SPSC channels to actor-based NVMe driver
- **Feature flags**: `spdk` (default), `testing` (exposes MockBlockDevice), `volatile_write_cache` (NVMe flush after checkpoint)
- **Workspace membership**: Excluded from `default-members`; requires explicit `-p extent-manager` builds

## Architecture

### Component Layer

```
+--------------------------------------------------------------+
|                     IExtentManager (trait)                    |
|  format | initialize | reserve_extent | remove_extent        |
|  checkpoint | get_extents | for_each_extent | ...            |
+--------------------------------------------------------------+
|                     ExtentManager                             |
|  +--------+  +--------+  +--------+  ...  +--------+        |
|  |Region 0|  |Region 1|  |Region 2|       |Region N|        |
|  |RwLock   |  |RwLock   |  |RwLock   |       |RwLock   |     |
|  +--------+  +--------+  +--------+       +--------+        |
|       |                                                      |
|  SharedState (Mutex): FormatParams, Superblock, ckpt_seq     |
|  CheckpointCoalesce (Mutex+Condvar): serializes ckpt I/O     |
|  CheckpointTimerState (Arc): background ckpt thread sleep    |
+--------------------------------------------------------------+
        |                              |
        v                              v
  [IBlockDevice receptacle]     [ILogger receptacle]
  (metadata NVMe device)        (structured logging)
```

### Internal Module Structure

```
src/
  lib.rs             -- ExtentManager struct, define_component!, IExtentManager impl
  superblock.rs      -- Superblock struct, serialize/deserialize, magic/version constants
  checkpoint.rs      -- write_checkpoint, read_checkpoint_region, SlabDescriptor, serde
  recovery.rs        -- recover() with dual-copy fallback, slab_from_descriptor()
  region.rs          -- RegionState (slabs, buddy, size_classes, pending_frees), SharedState
  buddy.rs           -- BuddyAllocator: alloc, free, mark_allocated, coalescing
  slab.rs            -- Slab (bitmap + key vec + rover), SizeClassManager, FREE_KEY
  bitmap.rs          -- AllocationBitmap: 64-bit word compact bitmap with roving find
  block_io.rs        -- BlockDeviceClient: sector-aligned read/write over SPSC channels
  error.rs           -- Error constructors mapping to ExtentManagerError variants
  write_handle.rs    -- (delegated to interfaces crate WriteHandle definition)
  test_support.rs    -- MockBlockDevice, MockLogger, heap_dma_alloc, create_test_component

tests/
  lifecycle.rs       -- User stories 1-3, 6-7: reserve/publish/abort/remove/format/enumerate
  checkpoint.rs      -- User stories 4-5: checkpoint persistence, recovery, crash fallback
  concurrent.rs      -- User story 8: multi-threaded stress (800 ops)
  edge_cases.rs      -- User story 9 + parameter validation + sentinel behavior

benches/
  benchmarks.rs      -- Criterion: reserve_publish, enumerate, remove, checkpoint
```

### Data Flow / Key Paths

**Allocate (reserve_extent)**:
1. Route key to region via `key & (region_count - 1)`
2. Acquire region write lock
3. Query `SizeClassManager` for a non-full slab of matching element_size
4. If no slab available: allocate slab-sized block from `BuddyAllocator`, create new `Slab`
5. Call `slab.alloc_slot()` using roving pointer for O(1) average allocation
6. Release region lock
7. Return `WriteHandle` with publish/abort closures capturing `(slab_start, slot_idx)`

**Publish (WriteHandle::publish)**:
1. Acquire region write lock
2. Set key in slab key vector at `slot_idx`
3. Mark region dirty
4. Return `Extent { key, offset, size }`

**Checkpoint**:
1. Coalescing gate: acquire `checkpoint_coalesce` mutex, wait if another in-progress
2. Check if any region is dirty -- skip if all clean
3. Serialize all regions: for each slab, write `(start_offset, slab_size, element_size, keys[])`
4. Compute inactive copy offset: `checkpoint_region_offset + (1 - active_copy) * region_size`
5. Build blob: `[seq:u64 | payload_len:u32 | crc32:u32 | payload...]`, compute CRC
6. Write blob to inactive checkpoint region (sector-aligned)
7. Optional flush (volatile_write_cache feature)
8. Flip superblock `active_copy` and increment `checkpoint_seq`
9. Write updated superblock to LBA 0
10. Optional flush
11. Clear dirty flags, call `flush_pending_frees()` on each region (deferred free)

**Recover (initialize)**:
1. Read superblock from LBA 0, validate magic + CRC
2. If `checkpoint_seq == 0`: empty state (fresh format)
3. Read active checkpoint region, validate seq + CRC
4. On failure: log warning, read inactive region with `seq - 1`
5. On both-corrupt: return `CorruptMetadata` error
6. Deserialize slab descriptors per region
7. Rebuild BuddyAllocators with `mark_allocated` for each recovered slab
8. Reconstruct Slab objects from descriptors (bitmap from key vector)

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| Dual-copy alternating checkpoint | Crash at any point during checkpoint write cannot corrupt the previously valid copy. The superblock pointer flip is the atomic commit point (single 4 KiB sector write). |
| Deferred free (pending_frees) | `remove_extent()` marks key as FREE_KEY but does not deallocate the slab slot until after the next successful checkpoint. This prevents offset reuse before the removal is durable -- a crash before checkpoint still has the extent. |
| Region sharding with power-of-two count | Eliminates modulo division (uses bitwise AND). Each region has an independent RwLock, allowing parallel allocation across different hash buckets with no global serialization. |
| Buddy allocator for slab allocation | Provides efficient coarse-grained space management with O(log N) allocation and automatic coalescing on free, preventing fragmentation of slab-sized blocks. |
| Roving pointer in slab | Distributes slot allocations evenly across the bitmap, avoiding hotspots at low indices and providing O(1) amortized allocation for non-full slabs. |
| Background checkpoint thread holds Weak<Self> | Prevents reference cycle: the timer thread only holds Arc<CheckpointTimerState> during sleep. Work phase upgrades Weak<ExtentManager>. Component Drop signals shutdown and joins the thread cleanly. |
| Checkpoint coalescing | At most one checkpoint I/O is in-progress. Concurrent callers wait on a Condvar and are satisfied by the in-flight or next checkpoint, reducing redundant I/O under contention. |
| SizeClassManager | Maps element sizes to lists of non-full slab offsets. Enables O(1) lookup for the next available slot of a given size without scanning all slabs. |
| CRC32 on both superblock and checkpoint | Detects bit-rot and torn writes. Recovery validates integrity before trusting data. |

## Disk Layout

```
Metadata Device:
+-------------------+----------------------------+----------------------------+
| Superblock (4 KiB)| Checkpoint Region A        | Checkpoint Region B        |
| LBA 0             | (aligned to metadata_align)| (immediately after A)      |
+-------------------+----------------------------+----------------------------+
                    ^                            ^
                    checkpoint_region_offset     offset + checkpoint_region_size

Data Device (or co-located after metadata):
+------------------+------------------+-----+------------------+
| Region 0         | Region 1         | ... | Region N-1       |
| (buddy-managed)  | (buddy-managed)  |     | (buddy-managed)  |
+------------------+------------------+-----+------------------+
^
data_start_offset (0 if separate device; metadata_region_size if co-located)

Superblock (4096 bytes):
  [0..8]   magic: 0x4345_5254_5553_5634 ("CERTUSV4")
  [8..12]  version: 6
  [12..20] data_disk_size: u64
  [20..24] sector_size: u32
  [24..32] slab_size: u64
  [32..36] max_extent_size: u32
  [36..40] region_count: u32
  [40..48] checkpoint_seq: u64
  [48]     active_copy: u8
  [49..56] reserved (7 bytes)
  [56..64] checkpoint_region_offset: u64
  [64..72] checkpoint_region_size: u64
  [72..80] instance_id: u64
  [80..84] metadata_disk_ns_id: u32
  [84..92] data_start_offset: u64
  [92..96] crc32: u32

Checkpoint Region:
  [0..8]   seq: u64
  [8..12]  payload_len: u32
  [12..16] crc32: u32 (over entire header+payload with this field zeroed)
  [16..]   payload:
            [0..4] region_count: u32
            per region:
              [0..4] num_slabs: u32
              per slab:
                [0..8]  start_offset: u64
                [8..16] slab_size: u64
                [16..20] element_size: u32
                [20..24] num_slots: u32
                [24..24+num_slots*8] keys: [u64; num_slots]
```

## Dependencies

| Crate | Role |
|-------|------|
| `component-core` | SpscChannel (used by MockBlockDevice in tests); core traits |
| `component-macros` | `define_component!` proc macro |
| `component-framework` | Facade re-export |
| `interfaces` (with `spdk` feature) | `IExtentManager`, `IBlockDevice`, `ILogger`, `FormatParams`, `WriteHandle`, `DmaBuffer`, channel types |
| `crc32fast` | CRC32 checksums for superblock and checkpoint integrity |
| `parking_lot` | High-performance `RwLock` for region-sharded concurrency |
| `criterion` (dev) | Benchmark harness |

## Testing

| Suite | Coverage |
|-------|----------|
| `tests/lifecycle.rs` | End-to-end: format, reserve, publish, abort, remove, enumerate, instance_id, used/capacity bytes |
| `tests/checkpoint.rs` | Checkpoint persistence, recovery after crash simulation, dual-copy fallback on corruption, deferred free semantics |
| `tests/concurrent.rs` | 8-thread stress: 800 concurrent reserve+publish, mixed ops, concurrent removes |
| `tests/edge_cases.rs` | Parameter validation (all invalid FormatParams combos), FREE_KEY sentinel, co-located metadata/data, base LBA offsets |
| `src/**/tests` | Unit tests per module: bitmap operations, buddy alloc/free/merge/mark, slab alloc/free/rover, superblock serde, checkpoint serde, size-class manager |
| `benches/benchmarks.rs` | Criterion: reserve_publish throughput, enumerate at scale, remove, checkpoint latency |

All tests run via `cargo test -p extent-manager` with the `testing` feature (auto-enabled by dev-dependencies). No hardware required -- `MockBlockDevice` simulates NVMe over in-memory hash map.

## Future Considerations

1. **Multi-device data sharding**: Currently data regions are on a single logical device. Future work could shard regions across multiple NVMe namespaces for bandwidth scaling.
2. **Incremental checkpoint**: Current implementation serializes all regions on every checkpoint. Large deployments could benefit from writing only dirty regions, reducing checkpoint I/O proportional to write rate.
3. **Online resize**: Adding regions or expanding buddy allocators without reformatting.
4. **Snapshot/clone support**: Copy-on-write extent semantics for point-in-time snapshots.
5. **Async I/O path**: The current synchronous block I/O (one command per sector) could batch writes for higher throughput on checkpoint.
6. **Tiered extent sizes**: Multiple slab sizes per region to reduce internal fragmentation for variable-size workloads.
7. **Formal verification expansion**: Current Creusot coverage (10 properties, 22 VCs) could extend to checkpoint atomicity and recovery correctness proofs.
