# Feature Specification: Dispatch Map Component

**Feature Branch**: `dispatch-map`  
**Created**: 2026-04-27  
**Status**: Complete  
**Input**: User description: "FUNCTIONAL-DESIGN.md — dispatch map component for the Certus storage system"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Staging Buffer Allocation for Incoming Data (Priority: P1)

A caller needs to write new data into the storage system. It requests a staging buffer from the dispatch map by providing an extent key and the desired size in 4KiB blocks. The dispatch map allocates a DMA-safe buffer, records the key in its internal map with a write reference, and returns the buffer so the caller can fill it with data.

**Why this priority**: This is the entry point for all new data flowing into the system. Without staging, no data can be ingested.

**Independent Test**: Can be fully tested by calling `create_staging(key, size)`, verifying that a DMA buffer is returned, that the key exists in the map with a write reference count of 1, and that duplicate `create_staging` calls for the same key while a write reference is held are rejected.

**Acceptance Scenarios**:

1. **Given** an empty dispatch map, **When** `create_staging(key=42, size=4)` is called, **Then** a DMA buffer of 4 x 4KiB is returned and the entry has write_ref=1.
2. **Given** key 42 already has a write reference, **When** `create_staging(key=42, size=4)` is called again, **Then** the call blocks or returns an error because a write lock is already held.
3. **Given** key 42 has a read reference, **When** `create_staging(key=42, size=4)` is called, **Then** the call blocks until all read references are released.

---

### User Story 2 - Looking Up Cached Data by Key (Priority: P1)

A caller needs to read extent data. It looks up an extent key in the dispatch map. The map determines whether the data is in a staging buffer or has been committed to block-device storage, acquires a read reference (blocking if a write is in progress), and returns the location so the caller can initiate a data transfer.

**Why this priority**: Read-path lookup is the primary hot path for inference workloads. Correctness and concurrency of reads directly affect system throughput.

**Independent Test**: Can be tested by first staging data for a key, then calling `lookup(key)` and verifying the correct location type is returned and the read reference count is incremented.

**Acceptance Scenarios**:

1. **Given** key 42 is staged with a DMA buffer and the write reference has been released, **When** `lookup(key=42)` is called, **Then** the DMA buffer pointer is returned and read_ref is incremented.
2. **Given** key 42 has been committed to block-device storage at offset 8192, **When** `lookup(key=42)` is called, **Then** `BlockDeviceLocation(offset=8192)` is returned and read_ref is incremented.
3. **Given** key 99 does not exist, **When** `lookup(key=99)` is called, **Then** `NotExist` is returned.
4. **Given** key 42 is looked up with size mismatch, **When** the caller expects a different size than recorded, **Then** `ErrorMismatchSize` is returned.
5. **Given** key 42 currently has an active write reference, **When** `lookup(key=42)` is called, **Then** the call blocks until write_ref reaches 0, then returns the data location with read_ref incremented.

---

### User Story 3 - Committing Staged Data to Persistent Storage (Priority: P2)

After data has been written to a staging buffer and flushed to the block device, the caller tells the dispatch map to record the on-disk location. This transitions the entry from a staging buffer to a block-device offset, allowing the staging buffer to be freed.

**Why this priority**: Persistence is essential for crash recovery, but it follows the write path established by staging.

**Independent Test**: Can be tested by staging a key, then calling `convert_to_storage(key, offset)` and verifying that subsequent lookups return `BlockDevice` location.

**Acceptance Scenarios**:

1. **Given** key 42 is staged, **When** `convert_to_storage(key=42, offset=8192)` is called, **Then** the entry location changes to `BlockDevice(offset=8192)`, the read reference count is decremented by 1, and the staging buffer can be freed.
2. **Given** key 42 does not exist, **When** `convert_to_storage(key=42, offset=8192)` is called, **Then** an error is returned.

---

### User Story 4 - Reference Counting for Concurrent Access (Priority: P1)

Multiple callers access the same extent concurrently. The dispatch map enforces a readers-writer lock semantic: multiple concurrent readers are allowed when no writer is active, and a writer blocks until all readers and other writers have finished. This prevents data corruption during concurrent access.

**Why this priority**: Thread safety is fundamental to the component's correctness in a multi-threaded inferencing workload.

**Independent Test**: Can be tested by acquiring read references from multiple threads, verifying they all succeed, then attempting a write reference from another thread and verifying it blocks until reads are released.

**Acceptance Scenarios**:

1. **Given** key 42 has write_ref=0 and read_ref=0, **When** `take_read(key=42)` is called, **Then** read_ref becomes 1 and the call returns immediately.
2. **Given** key 42 has read_ref=3 and write_ref=0, **When** `take_write(key=42)` is called, **Then** the call blocks until read_ref=0, then sets write_ref=1.
3. **Given** key 42 has write_ref=1, **When** `take_read(key=42)` is called, **Then** the call blocks until write_ref=0.
4. **Given** key 42 has write_ref=1 and read_ref=0, **When** `downgrade_reference(key=42)` is called, **Then** write_ref becomes 0 and read_ref becomes 1 atomically.
5. **Given** key 42 has read_ref=2, **When** `release_read(key=42)` is called, **Then** read_ref becomes 1.
6. **Given** key 42 has write_ref=1, **When** `release_write(key=42)` is called, **Then** write_ref becomes 0 and any blocked readers or writers are unblocked.
7. **Given** key 42 has write_ref=1 that is never released, **When** `take_read(key=42, timeout=100ms)` is called, **Then** a timeout error is returned after 100ms.
8. **Given** key 42 has read_ref=1 that is never released, **When** `take_write(key=42, timeout=100ms)` is called, **Then** a timeout error is returned after 100ms.

---

### User Story 5 - Recovery on Initialization (Priority: P2)

When the dispatch map component starts up, it recovers the set of committed extents from persistent storage by iterating all extents via the `IExtentManager` receptacle. This repopulates the in-memory map so that previously persisted data is immediately available for lookup.

**Why this priority**: Recovery ensures durability across restarts, but is only exercised on startup.

**Independent Test**: Can be tested by populating an extent manager with known extents, initializing the dispatch map against it, and verifying all extents appear in the map with correct metadata.

**Acceptance Scenarios**:

1. **Given** the extent manager contains extents for keys [10, 20, 30], **When** the dispatch map initializes, **Then** `lookup(10)`, `lookup(20)`, and `lookup(30)` each return `BlockDeviceLocation` with the correct offset and size.
2. **Given** the extent manager is empty, **When** the dispatch map initializes, **Then** the map is empty and lookups return `NotExist`.

---

### User Story 6 - Removing an Extent from the Map (Priority: P3)

A caller removes an extent key from the dispatch map. The entry is deleted and subsequent lookups for that key return `NotExist`.

**Why this priority**: Removal is needed for eviction and garbage collection but is lower frequency than read/write paths.

**Independent Test**: Can be tested by staging or committing a key, calling `remove(key)`, and verifying the key no longer exists.

**Acceptance Scenarios**:

1. **Given** key 42 exists in the map with no active references, **When** `remove(key=42)` is called, **Then** the entry is deleted and `lookup(key=42)` returns `NotExist`.
2. **Given** key 99 does not exist, **When** `remove(key=99)` is called, **Then** an appropriate error or no-op occurs.
3. **Given** key 42 has active read or write references, **When** `remove(key=42)` is called, **Then** an error is returned and the entry remains in the map.

---

### User Story 7 - Touching an Entry to Refresh Eviction Priority (Priority: P3)

A caller wants to indicate that a cache entry is still in active use without performing any data transfer. The caller calls `touch(key)` to update the entry's timestamp counter, preventing it from being selected as an eviction victim.

**Why this priority**: Touch is needed for efficient eviction policies — without it, entries can only refresh their priority via a full lookup (which takes a read reference and may involve DMA).

**Independent Test**: Can be tested by staging entries, touching one, then calling `oldest_keys` and verifying the touched entry has a newer timestamp than untouched entries.

**Acceptance Scenarios**:

1. **Given** key 42 exists in the map, **When** `touch(key=42)` is called, **Then** the entry's TSC timestamp is updated and the call returns success. No reference counts are modified.
2. **Given** key 99 does not exist, **When** `touch(key=99)` is called, **Then** a `KeyNotFound` error is returned.

---

### User Story 8 - Querying Oldest Entries for Eviction (Priority: P3)

The dispatcher's eviction logic needs to identify the least-recently-used entries. It calls `oldest_keys(n)` to retrieve up to `n` keys sorted by ascending TSC (oldest first), then selects victims for removal.

**Why this priority**: Eviction is needed for bounded cache capacity but is a background management function, not on the hot data path.

**Independent Test**: Can be tested by staging entries in sequence, verifying order matches creation order, then touching one entry and verifying it moves to the newest position.

**Acceptance Scenarios**:

1. **Given** keys [1, 2, 3] were created in order, **When** `oldest_keys(2)` is called, **Then** keys [1, 2] are returned (oldest TSC first).
2. **Given** key 1 was subsequently looked up (refreshing its TSC), **When** `oldest_keys(2)` is called, **Then** keys [2, 3] are returned.
3. **Given** the map is empty, **When** `oldest_keys(5)` is called, **Then** an empty list is returned.

---

### Edge Cases

- `create_staging` with size=0 returns an error.
- DMA buffer allocation failure in `create_staging` returns an error; no entry is recorded in the map.
- `release_read` or `release_write` on a key with ref count already at 0 returns an error.
- `downgrade_reference` when no write reference is held returns an error.
- High contention (hundreds of threads) on a single key is handled by the blocking semantics of `take_read`/`take_write`; no special throttling or fairness guarantee is required for v0.
- `convert_to_storage` while a write reference is held by another thread: the caller performing the conversion is expected to hold the write reference itself; concurrent write references are prevented by `take_write` semantics.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST define a `CacheKey` type as `u64` for identifying extents in the dispatch map.
- **FR-002**: System MUST store per-entry metadata consisting of: location (a `Location` enum with variants `Staging`, `BlockDevice`, and `MemoryTier`), size in 4KiB blocks, a read reference count (`u32`), a write reference count (`u32`), and a TSC timestamp (`u64`). Reference counts are protected by a `Mutex`/`Condvar` pair for blocking semantics.
- **FR-003**: System MUST provide `create_staging(key, size)` that allocates a DMA-safe staging buffer, records the entry in the map with a write reference of 1, and returns the buffer. MUST return an error if size is 0 or if DMA buffer allocation fails.
- **FR-004**: System MUST provide `lookup(key)` that returns one of: `NotExist`, `Staging(DmaBuffer)`, `BlockDevice(offset)`, or `MemoryTier(pointer, size)`. On success, the read reference count MUST be incremented. The call MUST block if a write reference is active until write_ref reaches 0, using a hardcoded default timeout (2000ms). The `MismatchSize` variant exists in the return enum for future use but is not currently triggered. The entry's TSC timestamp is refreshed on each successful lookup. Note: the size is stored internally in the `DispatchEntry` but is not exposed in the `LookupResult::BlockDevice` variant.
- **FR-005**: System MUST provide `convert_to_storage(key, offset)` that transitions an entry's location from a staging buffer to a block-device offset. As a side effect, the read reference count is decremented by 1 (the caller's staging read reference is implicitly released). When called on a `MemoryTier` entry, it sets the `ssd_offset` field rather than fully transitioning to `BlockDevice`.
- **FR-006**: System MUST provide `take_read(key)` that waits until write_ref=0 (using a hardcoded default timeout of 2000ms), then increments read_ref. MUST return a timeout error if the condition is not met within the deadline.
- **FR-007**: System MUST provide `take_write(key)` that waits until both read_ref=0 and write_ref=0 (using a hardcoded default timeout of 2000ms), then increments write_ref. MUST return a timeout error if the condition is not met within the deadline.
- **FR-008**: System MUST provide `release_read(key)` that atomically decrements read_ref. MUST return an error if read_ref is already 0.
- **FR-009**: System MUST provide `release_write(key)` that atomically decrements write_ref. MUST return an error if write_ref is already 0.
- **FR-010**: System MUST provide `downgrade_reference(key)` that atomically transitions from a write reference to a read reference (write_ref decremented and read_ref incremented in a single atomic step). MUST return an error if no write reference is held.
- **FR-011**: System MUST provide `remove(key)` that deletes the entry from the map. The call MUST return an error if any read or write references are still active; the caller is responsible for draining all references before removal.
- **FR-012**: On initialization, the system MUST recover all committed extents by calling `IExtentManager::for_each_extent` and populating the map with their metadata.
- **FR-013**: All `IDispatchMap` methods MUST be thread-safe and re-entrant, allowing concurrent calls from multiple threads.
- **FR-014**: System MUST use the `ILogger` receptacle for info, debug, and error logging throughout the component.
- **FR-015**: System MUST be implemented as a component using `define_component!` with `IDispatchMap` as a provided interface and `ILogger` and `IExtentManager` as receptacles.
- **FR-016**: System MUST provide `touch(key)` that updates the entry's TSC timestamp without acquiring any reference. MUST return `KeyNotFound` if the key does not exist. MUST NOT block or modify reference counts.
- **FR-017**: System MUST provide `oldest_keys(n)` that returns up to `n` keys sorted by ascending TSC value (oldest first). Used by the dispatcher's eviction logic to identify victim entries. MUST be thread-safe.
- **FR-018**: The dispatch map MUST support a `MemoryTier` location variant with fields: `pointer: *mut u8`, `size: u32`, `ssd_offset: Option<u64>`. Two additional methods MUST be provided: `create_memory_tier_entry(key, pointer, size)` creates an entry with MemoryTier location (accepting `*mut u8` pointer and `u32` size); `convert_memory_tier_to_block(key, offset)` sets the `ssd_offset` field on a MemoryTier entry.
- **FR-019**: The dispatch map MUST provide a `set_dma_alloc(alloc)` method for injecting the DMA allocator used by `create_staging`.
- **FR-020**: The `initialize()` method MUST be an explicit public API call (not implicitly called during construction). It rebuilds the map from extent manager state via `IExtentManager::for_each_extent`.
- **FR-021**: When `convert_to_storage` is called on a `MemoryTier` entry, it MUST set the `ssd_offset` field rather than transitioning to `BlockDevice` location.

### Key Entities

- **CacheKey**: A `u64` value uniquely identifying an extent in the dispatch map.
- **Dispatch Entry**: Holds the location (`Location` enum), size in 4KiB blocks, read reference count (`u32`), write reference count (`u32`), and TSC timestamp (`u64`). Protected by `Mutex`/`Condvar`.
- **Location**: An enum with three variants: `Staging(Arc<DmaBuffer>)` for in-memory DMA buffers, `BlockDevice { offset: u64 }` for committed data on SSD, and `MemoryTier { pointer: *mut u8, size: u32, ssd_offset: Option<u64> }` for DRAM-cached entries. Note: `size_blocks` is stored on the `DispatchEntry`, not within the `Location` variant.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All committed extents are recoverable from persistent storage on component initialization — 100% of extents reported by the extent manager appear in the map after startup.
- **SC-002**: Concurrent readers accessing the same key experience no data corruption and no deadlocks under sustained multi-threaded access.
- **SC-003**: Write-to-read downgrade completes atomically with no window where the entry is unprotected (neither read-locked nor write-locked).
- **SC-004**: Per-entry metadata is kept compact. The `DispatchEntry` struct size varies by `Location` variant (the `Staging` variant includes an `Arc<DmaBuffer>`).
- **SC-005**: Lookup of a cached key completes without blocking when no writer is active.
- **SC-006**: All reference count operations (take_read, take_write, release_read, release_write, downgrade) maintain consistent counts under concurrent access — no reference leaks or underflows.

## Clarifications

### Session 2026-04-27

- Q: Is the entry lifecycle one-way (Staging → Storage) or can entries transition back to staging? → A: One-way. Staging → Storage only. To re-stage, caller must remove and re-create.
- Q: What happens when `remove()` is called on an entry with active read or write references? → A: Returns an error. Caller must drain all references before calling remove.
- Q: What is the error behavior for invalid reference operations (underflow, no-write downgrade, size=0)? → A: Return error for all invalid operations. No panics or silent no-ops.
- Q: Should `take_read`/`take_write` block indefinitely or support a timeout? → A: Hardcoded default timeout of 2000ms (`DEFAULT_TIMEOUT`). Methods do not accept a per-call timeout parameter; the constant is used for all blocking operations.

## Assumptions

- The caller is responsible for performing actual I/O to/from the DMA buffer and block device; the dispatch map only tracks metadata and locations.
- The `IExtentManager` receptacle is bound and initialized before the dispatch map's recovery phase runs.
- The `ILogger` receptacle is bound before any logging calls are made.
- DMA buffer allocation is provided by the SPDK environment (via `DmaAllocFn`); the dispatch map delegates allocation but does not manage SPDK initialization.
- A single dispatch map instance serves one storage namespace; multi-namespace support is out of scope for v0.
- `convert_to_storage` takes only `(key, offset)` — no `block_device_id` parameter. The component does not track which block device holds an extent.
- Size parameter in `create_staging` is measured in 4KiB blocks, consistent with the extent manager's granularity.
- The `MemoryTier` location variant supports a DRAM caching tier where data resides in host memory before being written through to SSD.
- `convert_to_storage` on a MemoryTier entry sets the `ssd_offset` field rather than transitioning to BlockDevice; `convert_memory_tier_to_block` is the explicit transition method.
