# Feature Specification: Extent Manager V2

**Feature Branch**: `001-extent-manager-v2`
**Created**: 2026-04-23
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior and intended design. Review carefully
> and update to reflect any desired changes.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Two-Phase Extent Allocation (Priority: P1)

A storage application reserves an extent of a given size for a
caller-chosen key, receives a disk offset, writes data to that offset,
then publishes the mapping so it becomes visible to lookups. If the
write fails, the application aborts the reservation and the space is
reclaimed. This write-before-commit pattern ensures that extent
mappings only become visible after the data they reference is on disk.

**Why this priority**: This is the core operation of the extent manager.
Every other feature depends on being able to allocate, publish, and
look up extents.

**Independent Test**: Create an ExtentManagerV2 with a mock block
device, format it, reserve an extent, publish it, then look it up
and verify key/offset/size match.

**Acceptance Scenarios**:

1. **Given** a formatted device, **When** the application calls
   `reserve_extent(key, size)`, **Then** it receives a WriteHandle
   with a valid disk offset and the sector-aligned size.
2. **Given** a WriteHandle, **When** the application calls `publish()`,
   **Then** `lookup_extent(key)` returns an Extent with the correct
   offset and size.
3. **Given** a WriteHandle, **When** the application calls `abort()`,
   **Then** `lookup_extent(key)` returns KeyNotFound and the space
   is available for reuse.
4. **Given** a WriteHandle that is dropped without calling `publish()`
   or `abort()`, **Then** the reservation is automatically aborted.
5. **Given** two concurrent reservations for the same key, **When**
   the second calls `publish()` after the first has already published,
   **Then** the second publish returns DuplicateKey.

---

### User Story 2 - Crash-Consistent Checkpointing (Priority: P1)

The application periodically checkpoints the extent index to disk so
that the mapping survives device reboots. After a successful
checkpoint, all published extents are durable. Extents published after
the last checkpoint are lost on crash, but internal consistency is
always maintained. A background thread automatically triggers
checkpoints on a configurable interval.

**Why this priority**: Without checkpointing, all extent mappings are
lost on restart. This is equally critical to allocation.

**Independent Test**: Format a device, publish extents, checkpoint,
simulate a reboot via `reboot_from()`, call `initialize()`, and verify
all checkpointed extents are recovered.

**Acceptance Scenarios**:

1. **Given** published extents and a successful `checkpoint()` call,
   **When** the device is rebooted and `initialize()` is called,
   **Then** all extents from the checkpoint are recovered with correct
   key/offset/size.
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

### User Story 3 - Recovery with Dual-Chain Fallback (Priority: P2)

On `initialize()`, the recovery module reads the superblock, follows
the current checkpoint chain, and rebuilds the in-memory index. If
the current chain is corrupt (media error, partial write), recovery
falls back to the previous checkpoint chain. This provides an extra
layer of resilience against single-checkpoint corruption.

**Why this priority**: Recovery is essential but depends on
checkpointing working first. The dual-chain fallback is a resilience
enhancement beyond the basic recovery path.

**Independent Test**: Format, publish extents, checkpoint twice (so
both current and previous chains exist), corrupt the first block of
the current chain, then initialize and verify the previous chain's
extents are recovered.

**Acceptance Scenarios**:

1. **Given** a device with a valid current checkpoint chain, **When**
   `initialize()` is called, **Then** the current chain is used and
   all its extents are restored.
2. **Given** a device where the current chain's first block is
   corrupt, **When** `initialize()` is called, **Then** the previous
   chain is used as fallback and its extents are restored.
3. **Given** a superblock with invalid magic, **When** `initialize()`
   is called, **Then** it returns CorruptMetadata with a message
   identifying the magic mismatch.
4. **Given** a superblock with a CRC mismatch, **When**
   `initialize()` is called, **Then** it returns CorruptMetadata.

---

### User Story 4 - Region-Sharded Concurrency (Priority: P2)

Extent keys (which are hashes with good distribution) are sharded
across N independent regions using `key & (region_count - 1)`. Each
region has its own lock, index, buddy allocator, and slab set.
Hot-path operations only touch the target region's lock, enabling
concurrent operations on different regions without contention.

**Why this priority**: Concurrency is critical for production
throughput but requires the core allocation and persistence to work
first.

**Independent Test**: Spawn multiple threads performing
reserve/publish/lookup on distinct keys, verify all operations
succeed and the final extent count matches expectations.

**Acceptance Scenarios**:

1. **Given** 8 regions and 8 threads each publishing 100 extents with
   unique keys, **When** all threads complete, **Then** exactly 800
   extents are present.
2. **Given** concurrent reserve and abort operations, **When** threads
   alternate between publish and abort, **Then** only published
   extents are visible and the final count is correct.
3. **Given** pre-seeded extents distributed across regions, **When**
   concurrent threads remove non-overlapping keys, **Then** all
   removals succeed and the final extent list is empty.

---

### User Story 5 - Extent Enumeration (Priority: P3)

The application enumerates all published extents, either by collecting
them into a Vec or by iterating with a callback. Reserved-but-
unpublished extents are not included.

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

The application removes a published extent by key. The extent is
immediately removed from the index (lookups return KeyNotFound), but
the underlying disk slot remains allocated until after the next
successful checkpoint. This deferred-free design prevents a crash-
consistency bug: if the slot were reused immediately and new data
written, a crash before the next checkpoint would recover the old
extent pointing at corrupted data. After the checkpoint persists the
removal, the slot is freed to the slab allocator, and if the slab
becomes empty, it is freed back to the buddy allocator.

**Why this priority**: Removal completes the CRUD lifecycle but is
less critical than create and read operations.

**Independent Test**: Publish an extent, checkpoint, remove it,
allocate a new extent of the same size, verify the new extent gets
a different offset (slot not reused). Then checkpoint and allocate
again to verify the old slot is now reusable.

**Acceptance Scenarios**:

1. **Given** a published extent with key K, **When**
   `remove_extent(K)` is called, **Then** it succeeds and
   `lookup_extent(K)` returns KeyNotFound.
2. **Given** no extent for key K, **When** `remove_extent(K)` is
   called, **Then** it returns KeyNotFound.
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

- What happens when the device is completely full?
  `reserve_extent` returns OutOfSpace.
- What happens with key 0 or key u64::MAX?
  Both are valid extent keys.
- What happens when `format()` is called with invalid parameters
  (e.g., `sector_size = 0`, `slab_size` not a multiple of
  `sector_size`, `region_count` not a power of two)?
  Returns CorruptMetadata with a descriptive message.
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
  interface.
- **FR-002**: `format()` MUST validate all FormatParams: `sector_size
  > 0`, `slab_size` is a multiple of `sector_size`, `max_element_size
  <= slab_size`, `metadata_block_size` is a multiple of `sector_size`,
  `region_count` is a positive power of two.
- **FR-003**: `format()` MUST write a superblock at LBA 0 containing
  format parameters, disk geometry, and a CRC32 checksum.
- **FR-004**: `initialize()` MUST read the superblock, validate its
  magic and CRC, recover the extent index from the checkpoint chain,
  and rebuild all in-memory state.

#### Extent Lifecycle

- **FR-005**: `reserve_extent(key, size)` MUST allocate a sector-
  aligned slot and return a WriteHandle with the disk byte offset.
  The extent MUST NOT be visible in the index until `publish()`.
- **FR-006**: `WriteHandle::publish()` MUST atomically insert the
  extent into the region's index. If the key already exists, it MUST
  return DuplicateKey.
- **FR-007**: `WriteHandle::abort()` or dropping the handle MUST
  release the allocated slot without inserting into the index.
- **FR-008**: `lookup_extent(key)` MUST return the Extent for a
  published key or KeyNotFound.
- **FR-009**: `remove_extent(key)` MUST remove the extent from the
  index. The underlying disk slot MUST NOT be freed until after the
  next successful checkpoint (deferred free). Once the checkpoint
  persists the removal, the slot is released to the slab allocator.
  If the slab becomes empty, it MUST be returned to the buddy
  allocator.
- **FR-010**: `get_extents()` MUST return all published extents.
  `for_each_extent()` MUST invoke the callback for each published
  extent.

#### Persistence & Recovery

- **FR-011**: `checkpoint()` MUST serialize all region indexes and
  slab descriptors into a linked chain of CRC32-protected chunks,
  rotate the superblock's chain pointers, and free the old previous
  chain.
- **FR-012**: `checkpoint()` MUST skip I/O if no region has been
  modified since the last checkpoint.
- **FR-013**: Concurrent `checkpoint()` calls MUST be coalesced: at
  most two actual checkpoint I/O operations execute regardless of
  how many callers request one.
- **FR-014**: A background thread MUST call `checkpoint()` at a
  configurable interval (default 5000 ms).
- **FR-015**: Recovery MUST attempt the current checkpoint chain
  first; if it is unreadable (CRC failure, media error), recovery
  MUST fall back to the previous chain.
- **FR-016**: After a successful checkpoint followed by a reboot,
  `initialize()` MUST restore all extents that were published before
  the checkpoint. Internal consistency MUST always be maintained.

#### Space Management

- **FR-017**: Each region MUST use a buddy allocator for coarse-
  grained allocation of slab-sized chunks from its contiguous byte
  range.
- **FR-018**: Each slab MUST use a bitmap allocator to pack same-
  size extents, with a rover for even distribution.
- **FR-019**: A size-class manager MUST index slabs by element size
  so that allocation finds a compatible slab in O(1).
- **FR-020**: Keys MUST be sharded to regions by
  `key & (region_count - 1)`. Because keys are hashes, this
  provides uniform distribution.

#### Concurrency

- **FR-021**: Each region MUST be independently locked
  (`parking_lot::RwLock`). Hot-path operations MUST only acquire the
  target region's lock.
- **FR-022**: The component MUST be Send + Sync and safe for
  concurrent use from multiple threads.

#### Crash Safety

- **FR-023**: After `remove_extent`, the freed disk slot MUST NOT be
  reallocated until the removal has been persisted by a successful
  checkpoint. This prevents a crash-after-reallocation scenario where
  recovery would restore the old extent pointing at overwritten data.

### Key Entities

- **ExtentKey** (`u64`): Caller-chosen logical identifier. Expected
  to be a hash value with good distribution across the key space.
- **Extent** (`{ key, offset, size }`): A published mapping from a
  logical key to a physical disk location and size.
- **WriteHandle**: RAII two-phase commit handle. Holds a reserved
  slot; call `publish()` to commit or `abort()` / drop to release.
- **FormatParams**: Configuration for `format()`. All size fields
  are in bytes: `slab_size` (u64), `max_element_size` (u32),
  `metadata_block_size` (u32), `sector_size` (u32), plus
  `region_count` (u32).
- **Superblock**: On-disk header at LBA 0 (4096 bytes). Contains
  format parameters, checkpoint chain pointers, sequence number,
  and CRC32 checksum. Magic: `0x4345_5254_5553_5632` ("CERTUSV2").
- **Checkpoint Chain**: Linked list of CRC32-protected chunks, each
  `metadata_block_size` bytes. Encodes the complete extent index and
  slab descriptors for all regions.
- **Region**: Independent shard with its own index, slab set, buddy
  allocator, and lock. Region count must be a power of two.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All extent lifecycle operations (reserve, publish,
  lookup, remove, abort) produce correct results as verified by
  unit and integration tests.
- **SC-002**: Checkpoint + recovery round-trip preserves 100% of
  published extents with correct key/offset/size.
- **SC-003**: Dual-chain fallback successfully recovers from
  single-chain corruption.
- **SC-004**: Concurrent operations from 8+ threads produce no
  data races, lost updates, or panics.
- **SC-005**: The component supports approximately 100 million
  extents on a 10 TB device with 128 KiB extent size (target
  scale).
- **SC-006**: Checkpoint coalescing limits concurrent checkpoint
  I/O to at most two active operations regardless of caller count.

## On-Disk Format Reference

### Superblock (LBA 0, 4096 bytes)

| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | magic (`0x4345_5254_5553_5632`) |
| 8 | 4 | version (2) |
| 12 | 8 | disk_size |
| 20 | 8 | current_index_lba |
| 28 | 8 | previous_index_lba |
| 36 | 4 | sector_size |
| 40 | 8 | slab_size |
| 48 | 4 | max_element_size |
| 52 | 4 | metadata_block_size |
| 56 | 4 | region_count |
| 60 | 8 | checkpoint_seq |
| 68 | 4 | CRC32 of bytes 0-67 |
| 72 | 4024 | zero padding |

### Checkpoint Chunk Header (36 bytes)

| Offset | Size | Field |
|--------|------|-------|
| 0 | 4 | magic (`0x434B4E4B` / "CKNK") |
| 4 | 8 | seq |
| 12 | 8 | prev_lba |
| 20 | 8 | next_lba |
| 28 | 4 | payload_len |
| 32 | 4 | CRC32 |

### Checkpoint Payload (concatenated across chain)

```
u32 region_count
per region:
    u32 num_index_entries
    per entry (20 bytes): u64 key, u64 offset, u32 size
    u32 num_slabs
    per slab (20 bytes): u64 start_offset, u64 slab_size, u32 element_size
```

## Assumptions

- Keys are hashes with good distribution; the component does not
  need to handle skewed key distributions.
- The block device provides sector-atomic writes (a single sector
  write either completes fully or not at all).
- The superblock fits in a single sector (4096 bytes).
- DMA-capable memory allocation is provided by the caller via
  `set_dma_alloc()` before I/O operations.
- The component manages a single block device; multi-device
  coordination is out of scope.
- Log rotation and compaction of the checkpoint chain are out of
  scope; the chain is fully rewritten on each checkpoint.
