# Feature Specification: Extent Manager

**Feature Branch**: `001-extent-manager`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The Extent Manager is a crash-consistent, fixed-size extent allocator for the Certus storage system. It maps logical extent keys (`u64`) to physical disk offsets using a region-sharded buddy+slab allocation scheme with dual-copy checkpoint persistence. The component implements a two-phase allocation protocol (reserve then publish/abort) that allows callers to write data to a reserved location before committing the mapping, providing atomicity semantics for extent creation.

The on-disk format uses a 4 KiB CRC32-protected superblock on a metadata device, with two alternating checkpoint regions for crash consistency. Regions divide the data space into power-of-two shards, each independently locked, enabling concurrent allocation across threads. Within each region, a buddy allocator manages coarse slab allocation while a slab allocator handles fine-grained fixed-size slot management. Background periodic checkpointing with coalescing ensures durability without excessive I/O overhead.

## User Scenarios & Testing

### User Story 1 - Reserve, Write, and Publish an Extent (Priority: P1)

As a storage system component, I want to allocate a fixed-size extent on disk and associate it with a logical key, so that I can store data at a known physical location and retrieve it later by key.

**Acceptance Scenarios**:

- **Given** a formatted and initialized extent manager, **when** I call `reserve_extent(key, size)`, **then** I receive a `WriteHandle` with `extent_offset()` pointing to a valid disk location and `extent_size() >= size` aligned to the sector boundary.
- **Given** a valid `WriteHandle`, **when** I call `publish()`, **then** the extent appears in `get_extents()` with the correct key and offset.
- **Given** multiple distinct keys, **when** I reserve and publish each, **then** all appear in enumeration with unique offsets.
- **Given** key 0, **when** I reserve and publish, **then** it is stored as a valid extent (key 0 is not special).
- **Given** key `u64::MAX` (the FREE_KEY sentinel), **when** I reserve and publish, **then** `publish()` returns `Ok` but the extent is silently discarded and does not appear in enumeration.

### User Story 2 - Abort a Reservation (Priority: P1)

As a storage system component, I want to cancel a pending extent reservation without committing it, so that failed writes do not leak disk space.

**Acceptance Scenarios**:

- **Given** a valid `WriteHandle`, **when** I call `abort()`, **then** the reserved slot is freed and does not appear in `get_extents()`.
- **Given** a valid `WriteHandle`, **when** it is dropped without calling `publish()` or `abort()`, **then** the reservation is automatically aborted (Drop triggers abort).
- **Given** an aborted reservation, **when** I subsequently reserve another extent, **then** the previously freed space can be reused.

### User Story 3 - Remove a Published Extent (Priority: P1)

As a storage system component, I want to delete a previously published extent by its disk offset, so that the space can be reclaimed.

**Acceptance Scenarios**:

- **Given** a published extent at offset O, **when** I call `remove_extent(O)`, **then** the extent no longer appears in `get_extents()`.
- **Given** no extent at offset X, **when** I call `remove_extent(X)`, **then** `OffsetNotFound(X)` is returned.
- **Given** a removed extent, **when** a checkpoint is performed, **then** the slot is physically freed and can be reused by subsequent reservations.
- **Given** a removed extent before checkpoint, **when** the system crashes, **then** on recovery the extent is still present (removal is not durable until checkpointed).

### User Story 4 - Checkpoint Metadata to Disk (Priority: P1)

As a storage system operator, I want extent metadata to be periodically persisted to disk, so that allocated extents survive system crashes.

**Acceptance Scenarios**:

- **Given** dirty regions (extents published or removed since last checkpoint), **when** `checkpoint()` is called, **then** all current state is serialized, CRC-protected, and written to the inactive checkpoint region; the superblock `active_copy` pointer is flipped.
- **Given** no dirty regions, **when** `checkpoint()` is called, **then** no I/O is performed (optimization: skip clean state).
- **Given** concurrent `checkpoint()` calls, **when** multiple threads invoke checkpoint simultaneously, **then** coalescing ensures at most one physical checkpoint operation is in progress and waiters are satisfied by the completed operation.
- **Given** a configured `set_checkpoint_interval(Some(duration))`, **when** the duration elapses, **then** a background thread automatically triggers a checkpoint.
- **Given** `set_checkpoint_interval(None)`, **when** time passes, **then** no automatic checkpoints occur.

### User Story 5 - Initialize and Recover from Disk (Priority: P1)

As a storage system component, I want to recover previously checkpointed extent state after a restart, so that extent mappings are durable across reboots.

**Acceptance Scenarios**:

- **Given** a freshly formatted device (checkpoint_seq == 0), **when** `initialize()` is called, **then** the component starts with empty regions (no extents).
- **Given** a device with a valid checkpoint, **when** `initialize()` is called, **then** all extents from the last checkpoint are recovered with correct keys and offsets.
- **Given** extents published but not checkpointed before crash, **when** `initialize()` is called, **then** those uncheckpointed extents are lost.
- **Given** a corrupt active checkpoint region (CRC mismatch), **when** `initialize()` is called, **then** recovery falls back to the inactive (previous) checkpoint and succeeds.
- **Given** both checkpoint copies corrupt, **when** `initialize()` is called, **then** `CorruptMetadata` error is returned.
- **Given** an invalid superblock magic, **when** `initialize()` is called, **then** `CorruptMetadata` error mentioning "magic" is returned.

### User Story 6 - Enumerate All Allocated Extents (Priority: P2)

As a storage system component, I want to list all currently allocated extents, so that I can rebuild in-memory lookup structures or perform garbage collection.

**Acceptance Scenarios**:

- **Given** N published extents, **when** `get_extents()` is called, **then** exactly N extents are returned with correct keys and offsets.
- **Given** no published extents, **when** `get_extents()` is called, **then** an empty vector is returned.
- **Given** reserved but not published extents, **when** `get_extents()` is called, **then** they do not appear (only published extents are visible).
- **Given** N published extents, **when** `for_each_extent(callback)` is called, **then** the callback is invoked exactly N times without heap allocation for the collection.

### User Story 7 - Format a New Device (Priority: P1)

As a storage system operator, I want to initialize a blank storage device with the extent manager format, so that it can be used for extent allocation.

**Acceptance Scenarios**:

- **Given** valid `FormatParams`, **when** `format()` is called, **then** a superblock with magic `CERTUSV4`, format version 6, and CRC32 is written to LBA 0, and regions are initialized with buddy allocators.
- **Given** `sector_size == 0`, **when** `format()` is called, **then** `CorruptMetadata` error is returned.
- **Given** `slab_size` not a multiple of `sector_size`, **when** `format()` is called, **then** `CorruptMetadata` error is returned.
- **Given** `max_extent_size > slab_size`, **when** `format()` is called, **then** `CorruptMetadata` error is returned.
- **Given** `region_count == 0` or not a power of two, **when** `format()` is called, **then** `CorruptMetadata` error is returned.
- **Given** a metadata device too small for checkpoint regions, **when** `format()` is called, **then** `CorruptMetadata` error is returned.

### User Story 8 - Concurrent Multi-threaded Access (Priority: P1)

As a high-performance storage system, I want multiple threads to allocate, publish, and remove extents simultaneously, so that throughput scales with parallelism.

**Acceptance Scenarios**:

- **Given** 8 threads each performing 100 reserve+publish operations concurrently, **when** all threads complete, **then** exactly 800 unique extents exist with no duplicates.
- **Given** 8 threads performing mixed reserve/publish/abort operations concurrently, **when** all threads complete, **then** the published count matches the expected count with no corruption.
- **Given** 8 threads concurrently removing 100 extents each from a pre-populated set, **when** all threads complete, **then** all targeted extents are removed.

### User Story 9 - Data/Metadata Co-location (Priority: P2)

As a system integrator, I want metadata and data to coexist on the same physical SSD with a bounded metadata region, so that single-device deployments are supported.

**Acceptance Scenarios**:

- **Given** `metadata_region_size > 0` in FormatParams, **when** formatting, **then** data extents begin at offsets >= `metadata_region_size` (data does not overlap metadata).
- **Given** a co-located device after format and checkpoint, **when** recovering on a new instance, **then** extent offsets are preserved above the metadata region boundary.

## Requirements

### Functional Requirements

- **FR-001**: The component shall implement the `IExtentManager` interface with two-phase allocation (reserve/publish or reserve/abort).
- **FR-002**: `reserve_extent(key, size)` shall return a `WriteHandle` with the disk offset and sector-aligned size, or `OutOfSpace` if allocation fails.
- **FR-003**: `WriteHandle::publish()` shall atomically commit the extent mapping (key to offset). `WriteHandle::abort()` and `Drop` without publish shall free the reserved slot.
- **FR-004**: `remove_extent(offset)` shall mark the extent for removal. The slot is physically freed only after the next successful checkpoint (deferred free for crash consistency).
- **FR-005**: `checkpoint()` shall serialize all region state (slab descriptors and key vectors), write to the inactive checkpoint region with a CRC32 header, then flip the superblock `active_copy` pointer.
- **FR-006**: `initialize()` shall read the superblock, validate magic/CRC, then read the active checkpoint region to recover all region state. If the active copy is corrupt, fall back to the inactive copy.
- **FR-007**: `format()` shall validate all parameters, compute checkpoint region layout, initialize buddy allocators for each region, write the superblock, and initialize in-memory state.
- **FR-008**: `get_extents()` and `for_each_extent()` shall enumerate all published (committed) extents, excluding reserved-but-unpublished and FREE_KEY sentinel entries.
- **FR-009**: `get_instance_id()` shall return the unique instance identifier from the superblock.
- **FR-010**: `set_checkpoint_interval()` shall configure the background checkpoint thread: `Some(duration)` enables periodic checkpointing, `None` disables it.
- **FR-011**: `used_bytes()` and `capacity_bytes()` shall report current allocation usage and total capacity across all regions.
- **FR-012**: `set_metadata_base_lba()` and `set_data_base_lba()` shall configure partition-relative LBA offsets for metadata and data I/O.
- **FR-013**: Region sharding shall use `key & (region_count - 1)` to deterministically route keys to regions.
- **FR-014**: The FREE_KEY sentinel (`u64::MAX`) shall be treated as a silent discard on publish -- the reservation succeeds but no extent is stored.

### Non-Functional Requirements

- **NFR-001**: Crash consistency -- a crash at any point during checkpoint must not corrupt previously checkpointed data. The dual-copy alternation scheme with deferred superblock pointer update guarantees this.
- **NFR-002**: Concurrency -- region-sharded locking with `parking_lot::RwLock` per region shall allow parallel allocation across different regions without global serialization.
- **NFR-003**: Checkpoint coalescing -- at most one checkpoint I/O shall be in progress at a time; concurrent callers wait for the in-progress checkpoint to complete or trigger the next one.
- **NFR-004**: Memory efficiency -- slot state is tracked with 64-bit word bitmaps; key vectors are `Vec<u64>` indexed by slot.
- **NFR-005**: Testability -- the `testing` feature flag exposes `test_support` module with `MockBlockDevice` and `heap_dma_alloc()` for in-memory testing without hardware.
- **NFR-006**: Performance -- Criterion benchmarks shall cover reserve/publish, enumerate, remove, and checkpoint operations.
- **NFR-007**: Sector alignment -- all extent sizes returned from `reserve_extent` are rounded up to the nearest sector boundary.
- **NFR-008**: Background thread safety -- the checkpoint timer thread holds only `Weak<ExtentManager>` during work and `Arc<CheckpointTimerState>` for sleeping, allowing clean component drop without thread deadlock.
- **NFR-009**: CRC32 integrity -- both superblock and checkpoint regions are protected by CRC32 checksums validated on read.

## Key Entities

| Entity | Description |
|--------|-------------|
| `ExtentManager` | Top-level component implementing `IExtentManager`. Owns regions, shared state, and background checkpoint thread. |
| `Superblock` | 4 KiB on-disk header at LBA 0. Contains magic (`CERTUSV4`), format version (6), geometry params, active_copy indicator, checkpoint region offsets, instance_id, and CRC32. |
| `RegionState` | Per-region mutable state: slab map (`BTreeMap<u64, Slab>`), size-class manager, buddy allocator, dirty flag, and pending-frees list. |
| `SharedState` | Cross-region shared state: `FormatParams`, checkpoint sequence number, and current `Superblock`. |
| `BuddyAllocator` | Power-of-two block allocator for coarse slab allocation within a region. Supports alloc, free, and mark_allocated (for recovery). |
| `Slab` | Fixed-element-size allocator with allocation bitmap, key vector, and roving pointer for O(1) average alloc. |
| `SizeClassManager` | Maps element sizes to lists of non-full slab offsets for fast slot lookup. |
| `AllocationBitmap` | Compact bitmap (64-bit words) tracking per-slot allocation state within a slab. |
| `WriteHandle` | Opaque handle returned by `reserve_extent`. Supports `publish()` to commit or `abort()`/Drop to cancel. |
| `Extent` | Published extent record: `{ key: u64, offset: u64, size: u32 }`. |
| `FormatParams` | Configuration parameters for format: disk sizes, slab/extent sizes, sector size, region count, alignment, namespace ID. |
| `BlockDeviceClient` | Internal wrapper around `IBlockDevice` client channels for sector-aligned read/write I/O. |
| `CheckpointTimerState` | Shared state for background checkpoint thread: interval, condvar, shutdown flag. |
| `CheckpointCoalesce` | Mutex-protected state for serializing and coalescing concurrent checkpoint requests. |
| `SlabDescriptor` | Serialized slab representation in checkpoint: start_offset, slab_size, element_size, key vector. |

## Dependencies

| Dependency | Type | Description |
|------------|------|-------------|
| `IBlockDevice` (metadata) | Receptacle | NVMe block device for superblock and checkpoint I/O. Provides `connect_client()`, `sector_size()`, `num_sectors()`. |
| `ILogger` | Receptacle | Structured logging for info/warn/error messages during format, recovery, and checkpoint operations. |
| `component-macros` | Build | `define_component!` macro for component boilerplate (IUnknown, receptacles, version). |
| `interfaces` | Build | Shared trait definitions (`IExtentManager`, `IBlockDevice`, `ILogger`, `DmaBuffer`, `FormatParams`, etc.). |
| `parking_lot` | Build | High-performance `RwLock` for region-sharded concurrency. |
| `crc32fast` | Build | CRC32 checksum computation for superblock and checkpoint integrity. |
| `component-core` | Build (testing) | `SpscChannel` used by `MockBlockDevice` in test support. |

## Success Criteria

1. All unit tests pass (`cargo test -p extent-manager`) including lifecycle, checkpoint, concurrent, and edge-case suites.
2. Round-trip correctness: reserve, publish, checkpoint, crash-simulate, recover, enumerate yields identical extent set.
3. Crash consistency: corruption of the active checkpoint region triggers automatic fallback to the inactive copy.
4. Concurrency: 8-thread stress tests (800 concurrent operations) complete without data corruption or panics.
5. Space reclamation: removed extents are reusable after checkpoint flushes pending frees.
6. Parameter validation: all invalid `FormatParams` combinations are rejected with appropriate errors.
7. Background checkpoint: configurable interval fires automatically; disabling prevents automatic checkpoints.
8. Sentinel handling: `FREE_KEY` (u64::MAX) publishes silently discard without corrupting state.
9. Criterion benchmarks run without regression on the tested platform.
10. Formal verification (Creusot): 10 properties, 22 verification conditions discharged for core invariants (P1-P10).

## Implementation Notes

- **Component version**: 0.3.0 (declared via `define_component!`).
- **On-disk format version**: 6 (constant `FORMAT_VERSION` in `superblock.rs`).
- **Superblock magic**: `0x4345_5254_5553_5634` ("CERTUSV4").
- **Superblock size**: 4096 bytes (one sector).
- **Default checkpoint interval**: 30 seconds (set in `new_inner()`).
- **Checkpoint header**: 16 bytes (u64 seq + u32 payload_len + u32 CRC32).
- **Slab header in checkpoint**: 24 bytes (u64 start_offset + u64 slab_size + u32 element_size + u32 num_slots), followed by num_slots * 8 bytes of key data.
- **Region routing**: `key as usize & (regions.len() - 1)` -- requires power-of-two region count.
- **Deferred free semantics**: `remove_extent()` marks the key as FREE_KEY and pushes to `pending_frees`. Actual slab/buddy deallocation occurs in `flush_pending_frees()` after checkpoint, preventing offset reuse before durability.
- **Feature flags**: `spdk` (default, enables SPDK interface dependencies), `testing` (exposes test_support module), `volatile_write_cache` (adds NVMe flush after checkpoint writes).
- **SPDK exclusion**: This crate is excluded from workspace `default-members`; must be built explicitly with `-p extent-manager`.
- **Rover allocation**: Slab uses a roving pointer (`rover`) to distribute allocations across slots, avoiding hotspots.
- **Buddy coalescing**: Free operations merge adjacent buddy blocks up to max_order, maintaining allocation efficiency over time.
