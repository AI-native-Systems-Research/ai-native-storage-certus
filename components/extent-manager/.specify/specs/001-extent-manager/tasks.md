# Tasks: Extent Manager

**Branch**: `001-extent-manager` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md) | **Plan**: [plan.md](plan.md)
**Status**: Backfilled -- all tasks reflect existing implementation state.

---

## Task Group 1: Core Data Structures

### Task 1.1: Allocation Bitmap
- **File**: `src/bitmap.rs`
- **Status**: Done
- **Description**: Implement `AllocationBitmap` with 64-bit word storage, O(1) `set`/`clear`/`is_set`, roving `find_free_from`, `is_all_free`, `count_set`, `num_slots`.
- **Acceptance**:
  - [x] Bitmap supports arbitrary slot counts (not just multiples of 64)
  - [x] `find_free_from` wraps around and returns `None` when full
  - [x] `count_set` accurately tracks allocation count via atomic counter
  - [x] Debug assertions prevent double-set and double-clear
- **Tests**: `src/bitmap.rs::tests` (6 unit tests)

### Task 1.2: Slab Allocator
- **File**: `src/slab.rs`
- **Status**: Done
- **Description**: Implement `Slab` struct: fixed-element-size allocator with allocation bitmap, key vector (`Vec<u64>`), and roving pointer for O(1) amortized slot allocation.
- **Acceptance**:
  - [x] `new(start_offset, slab_size, element_size)` computes `num_slots = slab_size / element_size`
  - [x] `alloc_slot()` returns `(slot_idx, byte_offset)` using rover; `None` when full
  - [x] `free_slot(idx)` clears bitmap and resets key to `FREE_KEY`
  - [x] `set_key`/`get_key` manage the per-slot key vector
  - [x] `is_empty()`, `is_full()`, `contains_offset()`, `slot_for_offset()` utilities
  - [x] `mark_slot_allocated()` for recovery path (set bitmap without allocation)
  - [x] Rover wraps around slot count for even distribution
- **Tests**: `src/slab.rs::tests` (7 unit tests)

### Task 1.3: Size Class Manager
- **File**: `src/slab.rs`
- **Status**: Done
- **Description**: Implement `SizeClassManager` mapping element sizes to lists of non-full slab offsets for O(1) slab lookup during allocation.
- **Acceptance**:
  - [x] `add_slab(element_size, start_offset)` registers a non-full slab
  - [x] `remove_slab(element_size, start_offset)` removes entry; cleans up empty size class
  - [x] `get_slabs(element_size)` returns slice of available slab offsets
- **Tests**: `src/slab.rs::tests::size_class_manager`

### Task 1.4: Buddy Allocator
- **File**: `src/buddy.rs`
- **Status**: Done
- **Description**: Implement power-of-two block allocator for coarse slab allocation within regions. Supports non-power-of-two total sizes via decomposition into multiple initial free blocks.
- **Acceptance**:
  - [x] `new(base_offset, total_usable_size, sector_size)` initializes free lists covering entire space
  - [x] Non-power-of-two sizes decomposed into largest-possible free blocks
  - [x] `alloc(size)` finds smallest sufficient order, splits larger blocks, returns absolute offset
  - [x] `free(abs_offset, size)` returns block and coalesces with buddy up to max_order
  - [x] `mark_allocated(abs_offset, size)` for recovery: splits larger blocks to mark exact range allocated
  - [x] `total_free()` and `total_usable_size()` for capacity reporting
  - [x] Base offset applied to all returned addresses
- **Tests**: `src/buddy.rs::tests` (10 unit tests)

---

## Task Group 2: Region and State Management

### Task 2.1: Region State
- **File**: `src/region.rs`
- **Status**: Done
- **Description**: Implement `RegionState` owning a `BuddyAllocator`, `BTreeMap<u64, Slab>`, `SizeClassManager`, dirty flag, and pending-frees list. Provides the core allocation/publish/remove/free operations.
- **Acceptance**:
  - [x] `alloc_extent(size)` aligns to sector size, finds or creates slab, returns `(slab_start, slot_idx, offset)`
  - [x] When existing slab is full, removes from size_classes and allocates new slab via buddy
  - [x] `publish_slot(slab_start, slot_idx, key)` sets key and marks dirty
  - [x] `free_slot(slab_start, slot_idx)` frees bitmap slot; removes empty slab and returns buddy block
  - [x] `free_slot` re-adds slab to size_classes when it transitions from full to partial
  - [x] `remove_extent_by_offset(offset)` finds containing slab via BTreeMap range query, validates slot, marks FREE_KEY, pushes to pending_frees, marks dirty
  - [x] `flush_pending_frees()` processes deferred frees after checkpoint
- **Dependencies**: Task 1.1, 1.2, 1.3, 1.4

### Task 2.2: Shared State
- **File**: `src/region.rs`
- **Status**: Done
- **Description**: Define `SharedState` holding `FormatParams`, `checkpoint_seq`, and current `Superblock` snapshot.
- **Acceptance**:
  - [x] Accessible via `Mutex<Option<SharedState>>` on ExtentManager
  - [x] Updated atomically during format, initialize, and checkpoint

---

## Task Group 3: Superblock and Disk Format

### Task 3.1: Superblock Serialization
- **File**: `src/superblock.rs`
- **Status**: Done
- **Description**: Implement `Superblock` struct with `serialize()` to 4096-byte buffer and `deserialize()` with CRC32 validation.
- **Acceptance**:
  - [x] Magic: `0x4345_5254_5553_5634` ("CERTUSV4")
  - [x] Format version: 6
  - [x] Fields: data_disk_size, sector_size, slab_size, max_extent_size, region_count, checkpoint_seq, active_copy, checkpoint_region_offset, checkpoint_region_size, instance_id, metadata_disk_ns_id, data_start_offset
  - [x] CRC32 computed over all preceding fields, stored at end of serialized data
  - [x] `deserialize()` validates magic first (returns "invalid superblock magic" error)
  - [x] `deserialize()` validates CRC (returns "CRC mismatch" error)
  - [x] Round-trip: serialize then deserialize yields identical struct
- **Tests**: `src/superblock.rs::tests` (3 unit tests)

### Task 3.2: Block Device Client
- **File**: `src/block_io.rs`
- **Status**: Done
- **Description**: Implement `BlockDeviceClient` wrapping SPSC channel-based I/O to the NVMe actor. Provides sector-aligned `read_blocks` and `write_blocks` with DMA buffer management.
- **Acceptance**:
  - [x] `write_blocks(lba, data)` pads to sector boundary, sends one WriteSync command per sector, waits for WriteDone completion
  - [x] `read_blocks(lba, num_bytes)` reads ceiling(num_bytes/sector_size) sectors, returns exact num_bytes
  - [x] `with_base_lba` applies partition-relative offset to all LBA operations
  - [x] `alloc_buffer` uses the DmaAllocFn for proper DMA-capable memory
  - [x] Propagates NVMe errors through ExtentManagerError
- **Dependencies**: interfaces crate (ClientChannels, Command, Completion, DmaBuffer)

---

## Task Group 4: Checkpoint System (Crash Consistency)

### Task 4.1: Checkpoint Serialization
- **File**: `src/checkpoint.rs`
- **Status**: Done
- **Description**: Implement `serialize_region()` to encode all slabs in a region, and `write_checkpoint()` to build the full checkpoint blob with CRC32 header and write to the inactive copy.
- **Acceptance**:
  - [x] Per-region serialization: num_slabs count, then per slab: start_offset (u64), slab_size (u64), element_size (u32), num_slots (u32), keys (u64 * num_slots)
  - [x] Full payload: region_count (u32), then each region's serialized data
  - [x] Checkpoint header: seq (u64) + payload_len (u32) + crc32 (u32) = 16 bytes
  - [x] CRC32 computed over entire header+payload with CRC field zeroed
  - [x] Writes to inactive copy offset: `checkpoint_region_offset + (1 - active_copy) * region_size`
  - [x] Blob padded to sector boundary for block device write
  - [x] After write: flips `active_copy`, increments `checkpoint_seq`
  - [x] Optional NVMe flush before superblock update (volatile_write_cache feature)
  - [x] Validates payload fits within checkpoint_region_size
- **Dependencies**: Task 2.1, 3.2
- **Tests**: `src/checkpoint.rs::tests` (2 unit tests)

### Task 4.2: Checkpoint Deserialization
- **File**: `src/checkpoint.rs`
- **Status**: Done
- **Description**: Implement `read_checkpoint_region()` and `deserialize_slabs()` for reading and parsing checkpoint data from disk.
- **Acceptance**:
  - [x] `read_checkpoint_region` reads header sector first, validates seq match and bounds
  - [x] Reads exact number of sectors needed for full payload (avoids reading entire region)
  - [x] Validates CRC32 of entire blob
  - [x] Returns payload bytes on success
  - [x] `deserialize_slabs()` parses region_count and per-region slab descriptors into `Vec<Vec<SlabDescriptor>>`
  - [x] Detects truncation at every boundary (region count, slab count, slab header, key vector)
- **Dependencies**: Task 3.2

### Task 4.3: Checkpoint Coalescing
- **File**: `src/lib.rs` (ExtentManager::checkpoint, CheckpointCoalesce)
- **Status**: Done
- **Description**: Implement checkpoint coalescing so that concurrent checkpoint requests are serialized and waiters are satisfied by the completed operation.
- **Acceptance**:
  - [x] At most one `run_checkpoint()` executes at a time
  - [x] Callers arriving while checkpoint is in-progress wait on Condvar
  - [x] Each caller computes a `needed` seq: if in-progress, needs `completed_seq + 2` (ensuring a fresh checkpoint runs after the current one); otherwise `completed_seq + 1`
  - [x] When in-progress checkpoint completes, all waiters wake and check if their `needed` is satisfied
  - [x] `run_checkpoint()` skips I/O if no regions are dirty

### Task 4.4: Deferred Free Semantics
- **File**: `src/region.rs` (flush_pending_frees), `src/lib.rs` (run_checkpoint)
- **Status**: Done
- **Description**: Implement deferred free so that removed extents are not physically deallocated until after the next successful checkpoint, preventing offset reuse before removal is durable.
- **Acceptance**:
  - [x] `remove_extent_by_offset` sets key to FREE_KEY and pushes `(slab_start, slot_idx)` to `pending_frees`
  - [x] `flush_pending_frees()` called after successful checkpoint write + superblock update
  - [x] Pending frees are processed via `free_slot()` which may remove empty slabs and return buddy blocks
  - [x] A crash before checkpoint preserves the extent (key was FREE_KEY in checkpoint data, but slot was still allocated in the bitmap -- on recovery, FREE_KEY slots are not marked allocated)
  - [x] After checkpoint + flush, the space is genuinely available for reuse

### Task 4.5: Background Checkpoint Thread
- **File**: `src/lib.rs` (CheckpointTimerState, new_inner, Drop)
- **Status**: Done
- **Description**: Implement background periodic checkpointing with configurable interval, clean shutdown, and Weak reference pattern.
- **Acceptance**:
  - [x] Thread spawned in `new_inner()` with default 30-second interval
  - [x] Thread holds `Arc<CheckpointTimerState>` for sleep and `Weak<ExtentManager>` for work
  - [x] Sleep uses Condvar wait_timeout; distinguishes timeout (do checkpoint) from notify (re-check interval)
  - [x] `set_checkpoint_interval(Some(dur))` updates interval and wakes thread
  - [x] `set_checkpoint_interval(None)` disables automatic checkpoints (thread waits indefinitely on Condvar)
  - [x] `Drop` sets shutdown flag, notifies Condvar, joins thread
  - [x] Thread exits cleanly when Weak fails to upgrade (component dropped)
  - [x] Non-fatal: logs error on checkpoint failure but does not crash thread (except NotInitialized which is silent)

---

## Task Group 5: Recovery

### Task 5.1: Recovery with Dual-Copy Fallback
- **File**: `src/recovery.rs`
- **Status**: Done
- **Description**: Implement `recover()` that reads superblock, attempts active checkpoint, falls back to inactive on corruption, and rebuilds in-memory state.
- **Acceptance**:
  - [x] Reads superblock from LBA 0, validates via `Superblock::deserialize()`
  - [x] If `checkpoint_seq == 0`: returns empty per-region data (fresh format, no extents)
  - [x] Computes active offset: `checkpoint_region_offset + active_copy * checkpoint_region_size`
  - [x] Attempts `read_checkpoint_region` on active copy with expected seq
  - [x] On success: deserializes slabs, returns (superblock, per_region_data)
  - [x] On failure: logs warning, tries inactive copy with `seq - 1`
  - [x] If both corrupt: returns `CorruptMetadata` error
  - [x] `slab_from_descriptor()` reconstructs Slab with correct bitmap state from key vector (FREE_KEY = not allocated)
- **Dependencies**: Task 4.2, 3.1

### Task 5.2: Initialize (Full Recovery Path)
- **File**: `src/lib.rs` (IExtentManager::initialize)
- **Status**: Done
- **Description**: Implement `initialize()` that orchestrates recovery and rebuilds all in-memory structures.
- **Acceptance**:
  - [x] Gets metadata client using configured ns_id (default 1)
  - [x] Calls `recovery::recover()` to get superblock + per-region slab descriptors
  - [x] Reconstructs `FormatParams` from superblock fields
  - [x] Computes region geometry (data_start_offset, region_bytes per region)
  - [x] For each region: creates BuddyAllocator, calls `mark_allocated` for each recovered slab
  - [x] Reconstructs RegionState with slabs and size_classes populated from descriptors
  - [x] Sets `regions` and `shared` state
  - [x] Logs "recovery_start" and "recovery_complete"
- **Dependencies**: Task 5.1, 2.1, 1.4

---

## Task Group 6: Format and Validation

### Task 6.1: Format with Parameter Validation
- **File**: `src/lib.rs` (IExtentManager::format)
- **Status**: Done
- **Description**: Implement `format()` that validates all parameters, computes checkpoint layout, initializes regions, writes superblock, and sets up in-memory state.
- **Acceptance**:
  - [x] Rejects `sector_size == 0`
  - [x] Rejects `slab_size` not a multiple of `sector_size`
  - [x] Rejects `max_extent_size > slab_size`
  - [x] Rejects `region_count == 0` or non-power-of-two
  - [x] Queries metadata device size via `num_sectors * sector_size`
  - [x] Computes checkpoint_region_offset with alignment (round up superblock size to metadata_alignment)
  - [x] Computes checkpoint_region_size: `(remaining / 2)` aligned down to sector boundary
  - [x] Rejects if checkpoint_region_size == 0 (device too small)
  - [x] Computes data_start_offset: 0 if separate device; after checkpoint regions if co-located (metadata_region_size > 0)
  - [x] Rejects if usable_data_size == 0
  - [x] Initializes BuddyAllocator per region with computed base and size (last region gets remainder)
  - [x] Generates random instance_id from /dev/urandom (or uses provided value)
  - [x] Creates and serializes Superblock, writes to LBA 0
  - [x] Sets regions and shared state

---

## Task Group 7: IExtentManager API Implementation

### Task 7.1: reserve_extent
- **File**: `src/lib.rs`
- **Status**: Done
- **Description**: Route key to region, allocate slot, return WriteHandle with publish/abort closures.
- **Acceptance**:
  - [x] Routes via `key & (regions.len() - 1)`
  - [x] Aligns size to sector boundary
  - [x] Returns WriteHandle with correct key, offset, aligned_size
  - [x] Publish closure: sets key in slab, marks dirty, returns Extent
  - [x] Publish closure for FREE_KEY: frees slot silently, returns Extent without storing
  - [x] Abort closure: frees slot
  - [x] Drop without publish/abort triggers abort (via WriteHandle Drop impl in interfaces)

### Task 7.2: remove_extent
- **File**: `src/lib.rs`
- **Status**: Done
- **Description**: Route offset to region, delegate to `remove_extent_by_offset`.
- **Acceptance**:
  - [x] Computes region index from offset relative to data_start_offset
  - [x] Returns OffsetNotFound if index >= region_count
  - [x] Delegates to RegionState::remove_extent_by_offset

### Task 7.3: get_extents and for_each_extent
- **File**: `src/lib.rs`
- **Status**: Done
- **Description**: Enumerate all published extents across all regions.
- **Acceptance**:
  - [x] `get_extents()` returns Vec<Extent> of all non-FREE_KEY slots
  - [x] `for_each_extent()` invokes callback per extent without heap allocation for collection
  - [x] Both return empty/no-op if component not initialized

### Task 7.4: Capacity and Configuration APIs
- **File**: `src/lib.rs`
- **Status**: Done
- **Description**: Implement `used_bytes`, `capacity_bytes`, `get_instance_id`, `set_checkpoint_interval`, `set_metadata_base_lba`, `set_data_base_lba`, `data_base_lba`.
- **Acceptance**:
  - [x] `used_bytes()`: sum of `total_usable_size - total_free` across all regions
  - [x] `capacity_bytes()`: sum of `total_usable_size` across all regions
  - [x] `get_instance_id()`: returns superblock.instance_id or NotInitialized
  - [x] `set_checkpoint_interval()`: delegates to CheckpointTimerState
  - [x] `set_metadata_base_lba()` / `set_data_base_lba()`: store values for partition-relative I/O

---

## Task Group 8: Error Handling

### Task 8.1: Error Constructors
- **File**: `src/error.rs`
- **Status**: Done
- **Description**: Implement typed error constructors mapping internal conditions to `ExtentManagerError` variants.
- **Acceptance**:
  - [x] `offset_not_found(offset)` -> `OffsetNotFound(offset)`
  - [x] `out_of_space()` -> `OutOfSpace`
  - [x] `not_initialized(msg)` -> `NotInitialized(msg)`
  - [x] `io_error(msg)` -> `IoError(msg)`
  - [x] `corrupt_metadata(msg)` -> `CorruptMetadata(msg)`
  - [x] `nvme_to_em(e)` -> `IoError(e.to_string())`

---

## Task Group 9: Testing Infrastructure

### Task 9.1: Mock Block Device
- **File**: `src/test_support.rs`
- **Status**: Done
- **Description**: Implement `MockBlockDevice` with in-memory HashMap storage, fault injection, and reboot simulation.
- **Acceptance**:
  - [x] Implements `IBlockDevice` trait (connect_client, sector_size, num_sectors, etc.)
  - [x] SPSC channel-based I/O processing in spawned thread
  - [x] ReadSync: returns stored block or zeros
  - [x] WriteSync: stores block in HashMap, respects fault config
  - [x] FaultConfig: `fail_all_writes`, `fail_after_n_writes`
  - [x] `shared_state()` returns Arc for cross-instance state sharing
  - [x] `reboot_from(shared_state)` creates new instance pointing to same storage (simulates crash+restart)
  - [x] WriteZeros: removes entries from map

### Task 9.2: Test Utilities
- **File**: `src/test_support.rs`
- **Status**: Done
- **Description**: Implement `MockLogger`, `heap_dma_alloc`, and `create_test_component` helper.
- **Acceptance**:
  - [x] `MockLogger` implements ILogger, prints to stderr
  - [x] `heap_dma_alloc()` returns DmaAllocFn using std::alloc with proper alignment and registry-based free
  - [x] `create_test_component(metadata_disk_size)` wires up ExtentManager with MockBlockDevice and MockLogger

---

## Task Group 10: Integration Tests

### Task 10.1: Lifecycle Tests
- **File**: `tests/lifecycle.rs`
- **Status**: Done
- **Description**: End-to-end tests for User Stories 1-3, 6-7.
- **Coverage**:
  - [x] reserve_publish_round_trip
  - [x] Multiple distinct keys with unique offsets
  - [x] Abort cancels reservation, space reused
  - [x] Drop without publish aborts
  - [x] Remove published extent
  - [x] Remove non-existent offset returns OffsetNotFound
  - [x] Enumerate returns only published extents
  - [x] for_each_extent matches get_extents
  - [x] Format and empty enumeration
  - [x] Key 0 is valid
  - [x] FREE_KEY sentinel silently discards

### Task 10.2: Checkpoint Tests
- **File**: `tests/checkpoint.rs`
- **Status**: Done
- **Description**: Tests for User Stories 4-5 (checkpoint persistence, recovery, crash fallback).
- **Coverage**:
  - [x] Checkpoint persists extents across recovery
  - [x] Uncheckpointed extents lost on crash
  - [x] Active copy corruption triggers fallback to inactive
  - [x] Both copies corrupt returns CorruptMetadata
  - [x] Fresh format (seq==0) recovers to empty state
  - [x] Deferred free: removed extents present after crash before checkpoint
  - [x] Deferred free: removed extents gone after checkpoint + recovery
  - [x] Checkpoint coalescing: concurrent callers satisfied by single I/O
  - [x] Skip clean checkpoint (no dirty regions)

### Task 10.3: Concurrency Tests
- **File**: `tests/concurrent.rs`
- **Status**: Done
- **Description**: Multi-threaded stress tests for User Story 8.
- **Coverage**:
  - [x] 8 threads x 100 reserve+publish: exactly 800 unique extents
  - [x] 8 threads mixed reserve/publish/abort: correct published count
  - [x] 8 threads concurrent remove from pre-populated set

### Task 10.4: Edge Case Tests
- **File**: `tests/edge_cases.rs`
- **Status**: Done
- **Description**: Parameter validation and co-location tests for User Stories 7, 9.
- **Coverage**:
  - [x] sector_size == 0 rejected
  - [x] slab_size not multiple of sector_size rejected
  - [x] max_extent_size > slab_size rejected
  - [x] region_count == 0 rejected
  - [x] region_count non-power-of-two rejected
  - [x] Metadata device too small rejected
  - [x] Co-located metadata/data: data starts after metadata region
  - [x] Base LBA offsets applied correctly

---

## Task Group 11: Performance Benchmarks

### Task 11.1: Criterion Benchmarks
- **File**: `benches/benchmarks.rs`
- **Status**: Done
- **Description**: Criterion-based benchmarks for core operations.
- **Coverage**:
  - [x] `reserve_publish`: throughput of reserve+publish cycle
  - [x] `enumerate`: get_extents at 1, 1K, 100K scale
  - [x] `remove`: remove_extent throughput
  - [x] `checkpoint`: checkpoint latency at various extent counts

---

## Task Group 12: Crash Consistency Invariants

These are not code tasks but document the invariants that the implementation must maintain. They are verified by checkpoint tests and (partially) by Creusot formal verification.

### Invariant C1: Superblock Atomicity
The superblock is exactly one sector (4 KiB). NVMe guarantees atomic sector writes. The superblock is the commit point for a checkpoint: it is written last, after the new checkpoint data is fully persisted.

### Invariant C2: Dual-Copy Safety
At any point in time, at least one checkpoint region contains valid data for seq <= `superblock.checkpoint_seq`. A crash during write to the inactive region cannot affect the active region.

### Invariant C3: Monotonic Sequence
`checkpoint_seq` only increases. The active_copy pointer and seq are updated together in the superblock. Recovery reads the superblock seq and validates the checkpoint region's seq matches.

### Invariant C4: Deferred Free Window
Between `remove_extent()` and the next successful `checkpoint() + flush_pending_frees()`, the slot remains allocated (bitmap set, key = FREE_KEY). If a crash occurs in this window, recovery from the last checkpoint will see the extent as still present (key was its original value at checkpoint time).

### Invariant C5: CRC Coverage
Every on-disk structure (superblock, checkpoint region) carries a CRC32. Any bit-flip or partial write is detected during deserialization. Recovery rejects corrupt data and falls back to the alternate copy.

### Invariant C6: No Reuse Before Durability
Space freed by `remove_extent` is not returned to the buddy allocator until after the next successful checkpoint. This prevents a scenario where a freed offset is reallocated to a new extent, the new extent is written, and a crash rolls back to a state where the old extent's key points to data that has been overwritten.

---

## Dependency Graph

```
Task 1.1 (Bitmap) ─────────────────┐
Task 1.2 (Slab) ───────────────────┤
Task 1.3 (SizeClassManager) ───────┼──> Task 2.1 (RegionState) ──┐
Task 1.4 (BuddyAllocator) ─────────┘                             |
                                                                  |
Task 3.1 (Superblock) ─────────────────────────────────┐         |
Task 3.2 (BlockDeviceClient) ──────────────────────────┤         |
                                                       v         v
                                            Task 4.1 (Checkpoint Write) ──┐
                                            Task 4.2 (Checkpoint Read) ───┤
                                                                          v
Task 2.2 (SharedState) ─────────────────────────> Task 5.1 (Recovery) ────┤
                                                  Task 5.2 (Initialize) ──┤
                                                                          v
Task 8.1 (Errors) ──────────────────────────> Task 6.1 (Format) ──────────┤
                                                                          v
                                              Task 7.1 (reserve_extent) ──┤
                                              Task 7.2 (remove_extent) ───┤
                                              Task 7.3 (enumerate) ───────┤
                                              Task 7.4 (config APIs) ─────┤
                                              Task 4.3 (Coalescing) ──────┤
                                              Task 4.4 (Deferred Free) ───┤
                                              Task 4.5 (Background Ckpt) ─┤
                                                                          v
Task 9.1 (MockBlockDevice) ──┐                                           |
Task 9.2 (Test Utilities) ───┼──> Task 10.1-10.4 (Integration Tests) ────┤
                             |                                            v
                             └──> Task 11.1 (Benchmarks) ─────────────────┘
```

---

## Summary Statistics

| Metric | Value |
|--------|-------|
| Total tasks | 25 |
| Status: Done | 25 |
| Lines of source (src/) | ~950 |
| Lines of test (tests/) | ~600 |
| Unit test functions | ~30 |
| Integration test functions | ~30+ |
| Benchmark functions | 4 |
| Crash consistency invariants | 6 |
