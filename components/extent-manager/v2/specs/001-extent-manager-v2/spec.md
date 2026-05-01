# Feature Specification: Extent Manager V2

**Feature Branch**: `001-extent-manager-v2`
**Created**: 2026-04-23
**Updated**: 2026-04-29
**Status**: Active
**Source**: Generated from implementation, updated for index-free key-vector design

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Two-Phase Extent Allocation (Priority: P1)

A storage application reserves an extent of a given size for a
caller-chosen key, receives a disk offset, writes data to that offset,
then publishes the mapping so it becomes visible to enumeration. If the
write fails, the application aborts the reservation and the space is
reclaimed. This write-before-commit pattern ensures that extent
mappings only become visible after the data they reference is on disk.

**Why this priority**: This is the core operation of the extent manager.
Every other feature depends on being able to allocate, publish, and
enumerate extents.

**Independent Test**: Create an ExtentManagerV2 with mock block
devices, format it, reserve an extent, publish it, then verify the
extent appears in `get_extents()` with the correct key and offset.

**Acceptance Scenarios**:

1. **Given** a formatted device pair, **When** the application calls
   `reserve_extent(key, size)`, **Then** it receives a WriteHandle
   with a valid disk offset and the sector-aligned size.
2. **Given** a WriteHandle, **When** the application calls `publish()`,
   **Then** the extent appears in `get_extents()` with the correct
   key and offset.
3. **Given** a WriteHandle, **When** the application calls `abort()`,
   **Then** the extent is absent from `get_extents()` and the slot
   is available for reuse.
4. **Given** a WriteHandle that is dropped without calling `publish()`
   or `abort()`, **Then** the reservation is automatically aborted.
5. **Given** a reservation with key `u64::MAX` (the FREE_KEY sentinel),
   **When** `publish()` is called, **Then** it returns Ok but the
   extent is silently discarded and does not appear in `get_extents()`.

---

### User Story 2 - Crash-Consistent Checkpointing (Priority: P1)

The application periodically checkpoints the extent mapping to a
dedicated metadata disk so that the mapping survives device reboots.
The metadata disk contains a superblock followed by two contiguous
checkpoint regions that alternate. Checkpoints write to the inactive
region, then update the superblock to switch the active pointer. After
a successful checkpoint, all published extents are durable. Extents
published after the last checkpoint are lost on crash, but internal
consistency is always maintained.

**Why this priority**: Without checkpointing, all extent mappings are
lost on restart. This is equally critical to allocation.

**Independent Test**: Format a device pair, publish extents, checkpoint,
simulate a reboot via `reboot_from()`, call `initialize()`, and verify
all checkpointed extents are recovered via `get_extents()`.

**Acceptance Scenarios**:

1. **Given** published extents and a successful `checkpoint()` call,
   **When** the metadata device is rebooted and `initialize()` is
   called, **Then** all extents from the checkpoint are recovered with
   correct key/offset/size.
2. **Given** extents published after the last checkpoint, **When** the
   device is rebooted, **Then** those extents are not present after
   recovery (last-checkpoint consistency).
3. **Given** no changes since the last checkpoint, **When**
   `checkpoint()` is called, **Then** it completes successfully
   without writing to disk (skip-when-clean optimization).
4. **Given** multiple threads calling `checkpoint()` concurrently,
   **When** one checkpoint is already in progress, **Then** the other
   callers wait for the next completion rather than starting duplicate
   I/O (coalescing).

---

### User Story 3 - Recovery with Dual-Copy Fallback (Priority: P2)

On `initialize()`, the recovery module reads the superblock from the
metadata disk, reads the active checkpoint region, and rebuilds the
in-memory allocation state. If the active copy is corrupt (media error,
partial write), recovery falls back to the inactive copy. This provides
resilience against single-checkpoint corruption.

**Why this priority**: Recovery is essential but depends on
checkpointing working first. The dual-copy fallback is a resilience
enhancement beyond the basic recovery path.

**Independent Test**: Format, publish extents, checkpoint twice (so
both copies have been written), corrupt the active copy's first sector,
then initialize and verify the inactive copy's extents are recovered.

**Acceptance Scenarios**:

1. **Given** a metadata device with a valid active checkpoint copy,
   **When** `initialize()` is called, **Then** the active copy is used
   and all its extents are restored.
2. **Given** a metadata device where the active copy's first sector is
   corrupt, **When** `initialize()` is called, **Then** the inactive
   copy is used as fallback and its extents are restored.
3. **Given** a superblock with invalid magic, **When** `initialize()`
   is called, **Then** it returns CorruptMetadata with a message
   identifying the magic mismatch.
4. **Given** a superblock with a CRC mismatch, **When**
   `initialize()` is called, **Then** it returns CorruptMetadata.

---

### User Story 4 - Region-Sharded Concurrency (Priority: P2)

Extent keys (which are hashes with good distribution) are sharded
across N independent regions using `key & (region_count - 1)`. Each
region has its own lock, slab set, buddy allocator, and key vectors.
Hot-path operations only touch the target region's lock, enabling
concurrent operations on different regions without contention.

**Why this priority**: Concurrency is critical for production
throughput but requires the core allocation and persistence to work
first.

**Independent Test**: Spawn multiple threads performing
reserve/publish on distinct keys, verify all operations succeed and
the final extent count from `get_extents()` matches expectations.

**Acceptance Scenarios**:

1. **Given** 8 regions and 8 threads each publishing 100 extents with
   unique keys, **When** all threads complete, **Then** exactly 800
   extents are present in `get_extents()`.
2. **Given** concurrent reserve and abort operations, **When** threads
   alternate between publish and abort, **Then** only published
   extents are visible and the final count is correct.
3. **Given** pre-seeded extents distributed across regions, **When**
   concurrent threads remove non-overlapping extents (by offset),
   **Then** all removals succeed and the final extent list is empty.

---

### User Story 5 - Extent Enumeration (Priority: P3)

The application enumerates all published extents, either by collecting
them into a Vec or by iterating with a callback. Reserved-but-
unpublished extents are not included. Extents marked as removed (but
not yet freed by checkpoint) are also not included.

**Why this priority**: Enumeration supports diagnostics and
higher-level operations but is not on the critical allocation path.

**Independent Test**: Publish several extents, verify `get_extents()`
returns exactly the published set, and verify reserved-but-unpublished
handles are excluded.

**Acceptance Scenarios**:

1. **Given** 10 published extents, **When** `get_extents()` is called,
   **Then** it returns exactly 10 extents with correct keys.
2. **Given** a freshly formatted device with no published extents,
   **When** `get_extents()` is called, **Then** it returns an empty
   Vec.
3. **Given** outstanding (unpublished) WriteHandles, **When**
   `get_extents()` is called, **Then** the reserved extents are not
   included.
4. **Given** 5 published extents, **When** `for_each_extent()` is
   called with a counting callback, **Then** the callback is invoked
   exactly 5 times.

---

### User Story 6 - Extent Removal with Deferred Free (Priority: P3)

The application removes a published extent by supplying its disk byte
offset (obtained from the publish result). The extent is immediately
hidden from enumeration (its key slot is set to FREE_KEY in memory),
but the underlying disk slot remains allocated until after the next
successful checkpoint. This deferred-free design prevents a crash-
consistency bug: if the slot were reused immediately and new data
written, a crash before the next checkpoint would recover the old
extent pointing at corrupted data. After the checkpoint persists the
removal, the slot is freed to the slab allocator, and if the slab
becomes empty, it is freed back to the buddy allocator.

**Why this priority**: Removal completes the CRUD lifecycle but is
less critical than create and read operations.

**Independent Test**: Publish an extent, checkpoint, remove it by
offset, allocate a new extent of the same size, verify the new extent
gets a different offset (slot not reused). Then checkpoint and allocate
again to verify the old slot is now reusable.

**Acceptance Scenarios**:

1. **Given** a published extent at offset O, **When**
   `remove_extent(O)` is called, **Then** it succeeds and the extent
   no longer appears in `get_extents()`.
2. **Given** an offset O that does not correspond to any allocated
   extent, **When** `remove_extent(O)` is called, **Then** it returns
   OffsetNotFound.
3. **Given** a recently removed extent, **When** a new extent of the
   same size is reserved before the next checkpoint, **Then** the new
   extent MUST NOT be allocated the same disk slot as the removed one.
4. **Given** a removed extent, **When** a checkpoint completes
   successfully, **Then** the slot is freed and may be reused by
   subsequent allocations.
5. **Given** a removed extent and a crash before checkpoint, **When**
   recovery runs, **Then** the old extent is restored with its
   original data intact (the slot was never overwritten).

---

### Edge Cases

- What happens when the data device is completely full?
  `reserve_extent` returns OutOfSpace.
- What happens with key 0?
  Key 0 is a valid extent key.
- What happens with key `u64::MAX` (FREE_KEY)?
  `reserve_extent` succeeds normally. `publish()` succeeds and returns
  Ok, but the extent is silently discarded and does not appear in
  enumeration. The slot is immediately freed. This is the only key
  value with this special behavior.
- What happens when `format()` is called with invalid parameters
  (e.g., `sector_size = 0`, `slab_size` not a multiple of
  `sector_size`, `region_count` not a power of two)?
  Returns CorruptMetadata with a descriptive message.
- What happens when the metadata device is too small for two
  checkpoint regions?
  `format()` returns CorruptMetadata.
- What happens when operations are called before `format()` or
  `initialize()`?
  Returns NotInitialized.
- What happens when the component is dropped with outstanding
  WriteHandles?
  Does not panic; the background checkpoint thread is shut down
  gracefully.
- What happens when multiple size classes are needed (e.g., 4K, 8K,
  and 16K extents)?
  Each distinct sector-aligned size gets its own size class, and new
  slabs are allocated on demand per size class.
- What happens if an extent is removed and a new extent of the same
  size is immediately allocated, then the system crashes?
  The removed slot is not reused until after the next checkpoint.
  On recovery, the old extent is restored from the checkpoint with
  its original data intact. This deferred-free design prevents the
  new allocation from overwriting the old extent's disk region.

## Requirements *(mandatory)*

### Functional Requirements

#### Initialization & Format

- **FR-001**: The component MUST be named ExtentManagerV2 and defined
  using the `define_component!` macro, providing the IExtentManager
  interface with one IBlockDevice receptacle: `metadata_device`.
- **FR-002**: `format()` MUST validate all FormatParams: `sector_size
  > 0`, `slab_size` is a multiple of `sector_size`, `max_extent_size
  <= slab_size`, `region_count` is a positive power of two, metadata
  device is large enough for two checkpoint regions.
- **FR-003**: `format()` MUST write a superblock at LBA 0 of the
  metadata device containing format parameters, checkpoint region
  layout, and a CRC32 checksum.
- **FR-004**: `initialize()` MUST read the superblock from the
  metadata device, validate its magic and CRC, recover the extent
  mapping from the active checkpoint region, and rebuild all in-memory
  allocation state from the slab key vectors.

#### Extent Lifecycle

- **FR-005**: `reserve_extent(key, size)` MUST allocate a sector-
  aligned slot on the data device and return a WriteHandle with the
  disk byte offset. The extent MUST NOT be visible in enumeration
  until `publish()`.
- **FR-006**: `WriteHandle::publish()` MUST write the caller's key
  into the slot's position in the slab's key vector. If the key equals
  FREE_KEY (`u64::MAX`), the slot MUST be immediately freed and the
  call MUST return Ok without making any entry visible in enumeration.
- **FR-007**: `WriteHandle::abort()` or dropping the handle MUST
  release the allocated slot without writing any key.
- **FR-008**: `remove_extent(offset)` MUST set the key at the
  corresponding slab slot to FREE_KEY in memory. The extent MUST
  immediately cease to appear in `get_extents()` and `for_each_extent()`.
  If no allocated extent exists at `offset`, it MUST return
  OffsetNotFound. The underlying disk slot MUST NOT be freed until
  after the next successful checkpoint (deferred free). Once the
  checkpoint persists the removal, the slot is released to the slab
  allocator. If the slab becomes empty, it MUST be returned to the
  buddy allocator.
- **FR-009**: `get_extents()` MUST return all extents whose slab key
  slot is not FREE_KEY. `for_each_extent()` MUST invoke the callback
  for each such extent.

#### Per-Slab Key Vectors

- **FR-010**: Each `Slab` MUST maintain a dense `Vec<u64>` of keys
  parallel to its bitmap slots. This vector is the sole record of
  which keys occupy which disk slots; there is no separate per-region
  HashMap index.
- **FR-011**: The sentinel value `FREE_KEY = u64::MAX` MUST be used
  to mark unoccupied slots in the key vector. Any other value indicates
  an occupied slot belonging to that key.
- **FR-012**: Slabs within a region MUST be stored in a
  `BTreeMap<u64, Slab>` keyed by `start_offset` so that
  `remove_extent(offset)` can locate the owning slab in O(log n) via
  `range(..=offset).next_back()`.

#### Persistence & Recovery

- **FR-013**: `checkpoint()` MUST serialize, for each region, all slab
  descriptors and their complete key vectors into a contiguous
  CRC32-protected blob, write it to the inactive checkpoint region on
  the metadata device, then update the superblock to switch the active
  copy.
- **FR-014**: `checkpoint()` MUST skip I/O if no region has been
  modified since the last checkpoint.
- **FR-015**: Concurrent `checkpoint()` calls MUST be coalesced: at
  most two actual checkpoint I/O operations execute regardless of
  how many callers request one.
- **FR-016**: A background thread MUST call `checkpoint()` at a
  configurable interval (default 5000 ms).
- **FR-017**: Recovery MUST attempt the active checkpoint copy first;
  if it is unreadable (CRC failure, media error), recovery MUST fall
  back to the inactive copy.
- **FR-018**: After a successful checkpoint followed by a reboot,
  `initialize()` MUST restore all extents that were published before
  the checkpoint by reading each slab's key vector and marking any
  slot whose key is not FREE_KEY as allocated in the bitmap. Internal
  consistency MUST always be maintained.

#### Space Management

- **FR-019**: Each region MUST use a buddy allocator for coarse-
  grained allocation of slab-sized chunks from its contiguous byte
  range on the data device.
- **FR-020**: Each slab MUST use a bitmap allocator to pack same-
  size extents, with a rover for even distribution.
- **FR-021**: A size-class manager MUST index slabs by element size
  (using `start_offset` as the identifier) so that allocation finds
  a compatible slab in O(1).
- **FR-022**: Keys MUST be sharded to regions by
  `key & (region_count - 1)`. Because keys are hashes, this provides
  uniform distribution.

#### Concurrency

- **FR-023**: Each region MUST be independently locked
  (`parking_lot::RwLock`). Hot-path operations MUST only acquire the
  target region's lock.
- **FR-024**: The component MUST be Send + Sync and safe for
  concurrent use from multiple threads.

#### Crash Safety

- **FR-025**: After `remove_extent`, the freed disk slot MUST NOT be
  reallocated until the removal has been persisted by a successful
  checkpoint. This prevents a crash-after-reallocation scenario where
  recovery would restore the old extent pointing at overwritten data.

### Key Entities

- **ExtentKey** (`u64`): Caller-chosen logical identifier. Expected
  to be a hash value with good distribution across the key space.
  The value `u64::MAX` is reserved as FREE_KEY and cannot be stored.
- **FREE_KEY** (`u64::MAX`): Sentinel value in slab key vectors
  indicating an unoccupied slot. Publishing an extent with this key
  is a silent no-op.
- **Extent** (`{ key, offset, size }`): A published mapping from a
  logical key to a physical disk location and size. Produced by
  iterating via `get_extents()` or `for_each_extent()`.
- **WriteHandle**: RAII two-phase commit handle. Holds a reserved
  slot; call `publish()` to commit or `abort()` / drop to release.
- **FormatParams**: Configuration for `format()`. All size fields
  are in bytes: `data_disk_size` (u64), `slab_size` (u64),
  `max_extent_size` (u32), `sector_size` (u32),
  `metadata_alignment` (u64), plus `region_count` (u32).
- **Superblock**: On-disk header at LBA 0 of the metadata device
  (4096 bytes). Contains format parameters, active checkpoint
  indicator, checkpoint region layout, sequence number, and CRC32.
  Magic: `0x4345_5254_5553_5635` ("CERTUSV5"), version 5.
- **Checkpoint Region**: Contiguous CRC32-protected blob on the
  metadata device. Two copies alternate; the superblock records which
  is active. Encodes the slab table and key vectors for all regions.
  No separate index is stored.
- **Region**: Independent shard with its own slab BTreeMap, key
  vectors, buddy allocator, and lock. Region count must be a power
  of two.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All extent lifecycle operations (reserve, publish,
  enumerate, remove, abort) produce correct results as verified by
  unit and integration tests.
- **SC-002**: Checkpoint + recovery round-trip preserves 100% of
  published extents with correct key/offset/size, reconstructed
  entirely from slab key vectors.
- **SC-003**: Dual-copy fallback successfully recovers from
  single-copy corruption.
- **SC-004**: Concurrent operations from 8+ threads produce no
  data races, lost updates, or panics.
- **SC-005**: The component supports approximately 100 million
  extents on a 10 TB data device with 128 KiB extent size (target
  scale). At 1 GiB/slab this implies ~10,000 slabs; BTreeMap O(log n)
  lookup is efficient at this scale.
- **SC-006**: Checkpoint coalescing limits concurrent checkpoint
  I/O to at most two active operations regardless of caller count.

## On-Disk Format Reference

### Metadata Device Layout

```
[Superblock: 4096 bytes]
[Padding to metadata_alignment boundary]
[Checkpoint Copy 0: checkpoint_region_size bytes]
[Checkpoint Copy 1: checkpoint_region_size bytes]
```

Where:
- `checkpoint_region_offset = align_up(4096, metadata_alignment)`
- `checkpoint_region_size = (metadata_disk_size - checkpoint_region_offset) / 2`
  (rounded down to sector alignment)

### Superblock (LBA 0 of metadata device, 4096 bytes)

| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | magic (`0x4345_5254_5553_5635` = "CERTUSV5") |
| 8 | 4 | version (5) |
| 12 | 8 | data_disk_size |
| 20 | 4 | sector_size |
| 24 | 8 | slab_size |
| 32 | 4 | max_extent_size |
| 36 | 4 | region_count |
| 40 | 8 | checkpoint_seq |
| 48 | 1 | active_copy (0 or 1) |
| 49 | 7 | reserved (zero) |
| 56 | 8 | checkpoint_region_offset |
| 64 | 8 | checkpoint_region_size |
| 72 | 8 | instance_id |
| 80 | 4 | CRC32 of bytes 0-79 |
| 84 | 4012 | zero padding |

### Checkpoint Region Header (16 bytes)

| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | checkpoint_seq |
| 8 | 4 | payload_len |
| 12 | 4 | CRC32 (of header + payload, with CRC field zeroed) |

### Checkpoint Payload (follows header)

```
u32 region_count
per region:
    u32 num_slabs
    per slab:
        u64 start_offset
        u64 slab_size
        u32 element_size
        u32 num_slots          (= slab_size / element_size)
        u64[num_slots] keys    (FREE_KEY = u64::MAX means slot unoccupied)
```

The key vector for each slab is the complete allocation record.
There is no separate index of (key, offset, size) tuples.

### Data Device Layout

The entire data device is available for user extents. No superblock
or reserved regions. Each region's buddy allocator manages a
contiguous byte range of the data device.

## Assumptions

- Keys are hashes with good distribution; the component does not
  need to handle skewed key distributions.
- The block device provides sector-atomic writes (a single sector
  write either completes fully or not at all).
- The superblock fits in a single sector (4096 bytes).
- The DMA allocator defaults to `DmaBuffer::new()` (SPDK hugepage
  allocation). Tests may override it via `set_dma_alloc()` on the
  concrete `ExtentManagerV2` type. Callers of `IExtentManager` do
  not need to configure the allocator.
- A crash during checkpoint may corrupt the inactive (being-written)
  copy, but the active copy and superblock remain consistent.
