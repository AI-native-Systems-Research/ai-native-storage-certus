# Feature Specification: Dispatch Map Component

**Feature Branch**: `dispatch-map`  
**Created**: 2026-04-27  
**Status**: Complete  
**Last Synced**: 2026-07-22 — backfilled User Story 10/11, FR-025, FR-026 (`promote_block_to_memory_tier`, `try_evict_to_block`) from implemented, consumed code per `.specify/sync/drift-report.md`  
**Input**: User description: "FUNCTIONAL-DESIGN.md — dispatch map component for the Certus storage system"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Memory-Tier Entry Creation for Incoming Data (Priority: P1)

A caller needs to write new data into the storage system. It creates a memory-tier entry in the dispatch map by providing an extent key, an externally-allocated pointer to DRAM, and the size in bytes. The dispatch map records the key in its internal map with a write reference and tracks the pointer for subsequent lookups.

**Why this priority**: This is the entry point for all new data flowing into the system. Without memory-tier entry creation, no data can be ingested.

**Independent Test**: Can be fully tested by calling `create_memory_tier_entry(key, pointer, size)`, verifying that the key exists in the map with a write reference count of 1, and that duplicate calls for the same key while a write reference is held are rejected.

**Acceptance Scenarios**:

1. **Given** an empty dispatch map, **When** `create_memory_tier_entry(key=42, pointer, size=16384)` is called, **Then** the entry is created with write_ref=1 and MemoryTier location.
2. **Given** key 42 already exists in the map, **When** `create_memory_tier_entry(key=42, pointer, size=16384)` is called again, **Then** an `AlreadyExists` error is returned.
3. **Given** a null pointer is passed, **When** `create_memory_tier_entry(key=42, null, size=16384)` is called, **Then** an error is returned.

---

### User Story 2 - Looking Up Cached Data by Key (Priority: P1)

A caller needs to read extent data. It looks up an extent key in the dispatch map. The map determines whether the data is in the memory tier or has been committed to block-device storage, acquires a read reference (blocking if a write is in progress), and returns the location so the caller can initiate a data transfer.

**Why this priority**: Read-path lookup is the primary hot path for inference workloads. Correctness and concurrency of reads directly affect system throughput.

**Independent Test**: Can be tested by first creating a memory-tier entry for a key, then calling `lookup(key)` and verifying the correct location type is returned and the read reference count is incremented.

**Acceptance Scenarios**:

1. **Given** key 42 is a MemoryTier entry and the write reference has been released, **When** `lookup(key=42)` is called, **Then** the memory-tier pointer and size are returned and read_ref is incremented.
2. **Given** key 42 has been committed to block-device storage at offset 8192, **When** `lookup(key=42)` is called, **Then** `BlockDevice(offset=8192)` is returned and read_ref is incremented.
3. **Given** key 99 does not exist, **When** `lookup(key=99)` is called, **Then** `NotExist` is returned.
4. **Given** key 42 is looked up with size mismatch, **When** the caller expects a different size than recorded, **Then** `ErrorMismatchSize` is returned.
5. **Given** key 42 currently has an active write reference, **When** `lookup(key=42)` is called, **Then** the call blocks until write_ref reaches 0, then returns the data location with read_ref incremented.

---

### User Story 3 - Recording Write-Through to Persistent Storage (Priority: P2)

After data in the memory tier has been written through to the block device, the caller tells the dispatch map to record the on-disk offset. For MemoryTier entries, this sets the `ssd_offset` field (enabling eviction) rather than immediately transitioning to BlockDevice. The explicit `convert_memory_tier_to_block` method performs the full transition when the memory-tier buffer is evicted.

**Why this priority**: Persistence is essential for crash recovery, but it follows the write path established by memory-tier entry creation.

**Independent Test**: Can be tested by creating a memory-tier entry for a key, then calling `convert_to_storage(key, offset)` and verifying that `ssd_offset` is set. Then calling `convert_memory_tier_to_block(key)` and verifying that subsequent lookups return `BlockDevice` location.

**Acceptance Scenarios**:

1. **Given** key 42 is a MemoryTier entry, **When** `convert_to_storage(key=42, offset=8192)` is called, **Then** the entry's `ssd_offset` is set to `Some(8192)` and the read reference count is conditionally decremented by 1 (only if read_ref > 0).
2. **Given** key 42 is a MemoryTier entry with `ssd_offset: Some(8192)`, **When** `convert_memory_tier_to_block(key=42)` is called, **Then** the entry transitions to `BlockDevice(offset=8192)`.
3. **Given** key 42 does not exist, **When** `convert_to_storage(key=42, offset=8192)` is called, **Then** an error is returned.
4. **Given** key 42 is already a BlockDevice entry, **When** `convert_to_storage(key=42, offset=8192)` is called, **Then** an error is returned.

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

When the dispatch map component starts up, it recovers the set of committed extents from persistent storage by iterating all extents via the `IExtentManager` receptacle. This repopulates the in-memory map so that previously persisted data is immediately available for lookup. If no `IExtentManager` is bound, initialization succeeds with an empty map.

**Why this priority**: Recovery ensures durability across restarts, but is only exercised on startup.

**Independent Test**: Can be tested by populating an extent manager with known extents, initializing the dispatch map against it, and verifying all extents appear in the map with correct metadata. Additionally, initializing without a bound extent manager should succeed with an empty map.

**Acceptance Scenarios**:

1. **Given** the extent manager contains extents for keys [10, 20, 30], **When** the dispatch map initializes, **Then** `lookup(10)`, `lookup(20)`, and `lookup(30)` each return `BlockDeviceLocation` with the correct offset and size.
2. **Given** the extent manager is empty, **When** the dispatch map initializes, **Then** the map is empty and lookups return `NotExist`.
3. **Given** no `IExtentManager` is bound, **When** the dispatch map initializes, **Then** initialization succeeds (returns `Ok(())`) with an empty map and lookups return `NotExist`.

---

### User Story 6 - Removing an Extent from the Map (Priority: P3)

A caller removes an extent key from the dispatch map. The entry is deleted and subsequent lookups for that key return `NotExist`.

**Why this priority**: Removal is needed for eviction and garbage collection but is lower frequency than read/write paths.

**Independent Test**: Can be tested by creating an entry for a key, calling `remove(key)`, and verifying the key no longer exists.

**Acceptance Scenarios**:

1. **Given** key 42 exists in the map with no active references, **When** `remove(key=42)` is called, **Then** the entry is deleted and `lookup(key=42)` returns `NotExist`.
2. **Given** key 99 does not exist, **When** `remove(key=99)` is called, **Then** an appropriate error or no-op occurs.
3. **Given** key 42 has active read or write references, **When** `remove(key=42)` is called, **Then** an error is returned and the entry remains in the map.

---

### User Story 7 - Touching an Entry to Refresh Eviction Priority (Priority: P3)

A caller wants to indicate that a cache entry is still in active use without performing any data transfer. The caller calls `touch(key)` to update the entry's timestamp counter, preventing it from being selected as an eviction victim.

**Why this priority**: Touch is needed for efficient eviction policies — without it, entries can only refresh their priority via a full lookup (which takes a read reference).

**Independent Test**: Can be tested by creating entries, touching one, then calling `oldest_keys` and verifying the touched entry has a newer timestamp than untouched entries.

**Acceptance Scenarios**:

1. **Given** key 42 exists in the map, **When** `touch(key=42)` is called, **Then** the entry's eviction priority is refreshed via `IEvictionPolicy` and the call returns success. No reference counts are modified.
2. **Given** key 99 does not exist, **When** `touch(key=99)` is called, **Then** a `KeyNotFound` error is returned.

---

### User Story 8 - Querying Oldest Entries for Eviction (Priority: P3)

The dispatcher's eviction logic needs to identify the least-recently-used entries. It calls `oldest_keys(n)` to retrieve up to `n` keys sorted by LRU order (oldest first), then selects victims for removal.

**Why this priority**: Eviction is needed for bounded cache capacity but is a background management function, not on the hot data path.

**Independent Test**: Can be tested by creating entries in sequence, verifying order matches creation order, then touching one entry and verifying it moves to the newest position.

**Acceptance Scenarios**:

1. **Given** keys [1, 2, 3] were created in order, **When** `oldest_keys(2)` is called, **Then** keys [1, 2] are returned (least recently used first).
2. **Given** key 1 was subsequently looked up (refreshing its eviction priority), **When** `oldest_keys(2)` is called, **Then** keys [2, 3] are returned.
3. **Given** the map is empty, **When** `oldest_keys(5)` is called, **Then** an empty list is returned.

---

### User Story 9 - Checking Evictability of a Memory-Tier Entry (Priority: P3)

The dispatcher's eviction logic needs to determine whether a specific memory-tier entry can be safely evicted. It calls `is_evictable(key)` which returns true only when the entry is in the `MemoryTier` state with a completed write-through (`ssd_offset` is `Some`) and no active references.

**Why this priority**: Eviction safety checks prevent data loss by ensuring entries are only evicted after their data has been persisted to SSD and no concurrent accessors hold references.

**Independent Test**: Can be tested by creating a memory-tier entry, verifying it is not evictable (write ref held), releasing the write ref and verifying still not evictable (no ssd_offset), setting ssd_offset via `convert_to_storage`, and then verifying it becomes evictable.

**Acceptance Scenarios**:

1. **Given** key 42 is a MemoryTier entry with `ssd_offset: Some(8192)` and read_ref=0 and write_ref=0, **When** `is_evictable(key=42)` is called, **Then** `true` is returned.
2. **Given** key 42 is a MemoryTier entry with `ssd_offset: None`, **When** `is_evictable(key=42)` is called, **Then** `false` is returned.
3. **Given** key 42 is a MemoryTier entry with `ssd_offset: Some(8192)` but read_ref=1, **When** `is_evictable(key=42)` is called, **Then** `false` is returned.
4. **Given** key 42 is a BlockDevice entry with read_ref=0 and write_ref=0, **When** `is_evictable(key=42)` is called, **Then** `false` is returned (only MemoryTier entries are evictable).
5. **Given** key 99 does not exist, **When** `is_evictable(key=99)` is called, **Then** `false` is returned.

---

### User Story 10 - Promoting a Cold Block-Device Entry Back to Memory Tier (Priority: P2)

The dispatcher, on a read miss for a block-device-resident extent, allocates a DRAM buffer, stages the data transfer from SSD, and then needs the dispatch map to reflect that the extent is now available in the memory tier — without disturbing any reference already held on the entry (the entry is typically pinned by the in-flight load itself). It calls `promote_block_to_memory_tier(key, pointer, size)`, which flips the entry from `BlockDevice` to `MemoryTier` **in place**: the eviction handle and all active read/write reference counts are preserved, and the original block-device offset is retained as the entry's `ssd_offset` so the promoted entry remains demotable back to `BlockDevice` without requiring a fresh write-through.

**Why this priority**: This is the read-miss promotion path that lets the dispatcher's SSD-resident cold data become memory-tier hot data for subsequent low-latency access. It is consumed by both `dispatcher` and `dispatcher-p2p` for on-demand cold-block promotion, but it follows the core read/write and reference-counting paths (P1) in priority.

**Independent Test**: Can be tested by inserting a `BlockDevice` entry (e.g. via `recover_extent`), optionally taking a read reference on it, calling `promote_block_to_memory_tier(key, pointer, size)`, and verifying the entry is now `MemoryTier` with `ssd_offset` equal to the original block offset, the reference count is unchanged, and a subsequent `lookup` returns the new memory-tier pointer/size.

**Acceptance Scenarios**:

1. **Given** key 42 is a `BlockDevice` entry at offset 8192, **When** `promote_block_to_memory_tier(key=42, pointer, size=16384)` is called, **Then** the entry transitions to `MemoryTier { pointer, size: 16384, ssd_offset: Some(8192) }`, `size_blocks` is recomputed from the new size, and the call returns `Ok(())`.
2. **Given** key 42 is a `BlockDevice` entry with an active read reference (`read_ref=1`, held by an in-flight load), **When** `promote_block_to_memory_tier(key=42, pointer, size=16384)` is called, **Then** the promotion succeeds in place and `read_ref` remains `1` afterward (the reference and eviction handle are preserved, not reset).
3. **Given** key 42 is already a `MemoryTier` entry, **When** `promote_block_to_memory_tier(key=42, pointer, size=16384)` is called, **Then** an `InvalidState` error is returned and the entry is left unchanged.
4. **Given** key 99 does not exist, **When** `promote_block_to_memory_tier(key=99, pointer, size=16384)` is called, **Then** a `KeyNotFound` error is returned.
5. **Given** `size=0` is passed, **When** `promote_block_to_memory_tier(key=42, pointer, size=0)` is called, **Then** an `InvalidSize` error is returned and the entry is left unchanged.

---

### User Story 11 - Atomically Evicting a Memory-Tier Entry to Block Device (Priority: P2)

The dispatcher's SSD-evictor background path needs to demote a memory-tier entry back to block-device state to free its DRAM slot, but only if it is safe to do so — no active references and write-through already complete. Rather than calling `is_evictable(key)` and `convert_memory_tier_to_block(key)` as two separate steps (which would leave a race window between the check and the transition), it calls `try_evict_to_block(key)`, which performs the check and the state transition atomically under a single lock hold. Once this call returns `Ok(())`, no new reader can obtain the memory-tier pointer, so the caller may safely free the DRAM slot.

**Why this priority**: This is the eviction-safety counterpart to `promote_block_to_memory_tier` and closes the loop on the memory-tier/block-device lifecycle used by the dispatcher's and dispatcher-p2p's SSD-evictor paths. It follows the core reference-counting and eviction paths (P1/P3) in priority.

**Independent Test**: Can be tested by creating a memory-tier entry, setting its `ssd_offset` via `convert_to_storage`, releasing all references, calling `try_evict_to_block(key)`, and verifying the entry is now `BlockDevice` at the expected offset. Separately, verify the call is rejected when references are active, when `ssd_offset` is not yet set, or when the entry is already `BlockDevice`.

**Acceptance Scenarios**:

1. **Given** key 42 is a `MemoryTier` entry with `ssd_offset: Some(8192)` and `read_ref == 0 && write_ref == 0`, **When** `try_evict_to_block(key=42)` is called, **Then** the entry transitions to `BlockDevice { offset: 8192 }` and the call returns `Ok(())`.
2. **Given** key 42 is a `MemoryTier` entry with `ssd_offset: Some(8192)` but `read_ref=1`, **When** `try_evict_to_block(key=42)` is called, **Then** an `InvalidState` error is returned and the entry remains `MemoryTier` (no partial transition).
3. **Given** key 42 is a `MemoryTier` entry with `ssd_offset: None` (write-through not complete) and no active references, **When** `try_evict_to_block(key=42)` is called, **Then** an `InvalidState` error is returned and the entry remains `MemoryTier`.
4. **Given** key 42 is already a `BlockDevice` entry, **When** `try_evict_to_block(key=42)` is called, **Then** an `InvalidState` error is returned.
5. **Given** key 99 does not exist, **When** `try_evict_to_block(key=99)` is called, **Then** a `KeyNotFound` error is returned.

---

### Edge Cases

- `create_memory_tier_entry` with a null pointer returns an error; no entry is recorded in the map.
- `create_memory_tier_entry` for an existing key returns `AlreadyExists`.
- `release_read` or `release_write` on a key with ref count already at 0 returns an error.
- `downgrade_reference` when no write reference is held returns an error.
- High contention (hundreds of threads) on a single key is handled by the blocking semantics of `take_read`/`take_write`; no special throttling or fairness guarantee is required for v0.
- `convert_to_storage` while a write reference is held by another thread: the caller performing the conversion is expected to hold the write reference itself; concurrent write references are prevented by `take_write` semantics.
- `recover_extent` for a key that already exists returns `AlreadyExists`.
- `promote_block_to_memory_tier` on an entry with active references preserves those references and the eviction handle across the transition (the entry may be pinned by an in-flight load).
- `promote_block_to_memory_tier` with `size=0` returns `InvalidSize` and leaves the entry unchanged.
- `try_evict_to_block` performs its evictability check and the `MemoryTier`→`BlockDevice` transition under a single lock hold, so no other thread can observe or acquire a reference between the check and the transition.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST define a `CacheKey` type as `u64` for identifying extents in the dispatch map.
- **FR-002**: System MUST store per-entry metadata consisting of: location (a `Location` enum with variants `BlockDevice` and `MemoryTier`), size in 4KiB blocks, a read reference count (`u32`), a write reference count (`u32`), and an `EvictionHandle` for LRU ordering. Reference counts are protected by a `Mutex`/`Condvar` pair for blocking semantics. Eviction ordering is delegated to the external `IEvictionPolicy` component via the eviction handle.
- **FR-003**: System MUST provide `create_memory_tier_entry(key, pointer, size)` that creates an entry with `MemoryTier` location (accepting a `*mut u8` pointer and `u32` size in bytes), records the entry in the map with a write reference of 1, and registers it with the eviction policy. MUST return `AlreadyExists` if the key is already present.
- **FR-004**: System MUST provide `lookup(key)` that returns one of: `NotExist`, `BlockDevice(offset)`, or `MemoryTier(pointer, size)`. On success, the read reference count MUST be incremented. The call MUST block if a write reference is active until write_ref reaches 0, using a hardcoded default timeout (2000ms). The `MismatchSize` variant exists in the return enum for future use but is not currently triggered. The entry's eviction priority is refreshed on each successful lookup. Note: the size is stored internally in the `DispatchEntry` but is not exposed in the `LookupResult::BlockDevice` variant.
- **FR-005**: System MUST provide `convert_to_storage(key, offset)` that records the on-disk offset for a MemoryTier entry by setting its `ssd_offset` field (enabling subsequent eviction). As a side effect, the read reference count is conditionally decremented by 1 (only if read_ref > 0); if read_ref is already 0, no decrement occurs. Returns an error if the key does not exist or the entry is already in `BlockDevice` state.
- **FR-006**: System MUST provide `take_read(key)` that waits until write_ref=0 (using a hardcoded default timeout of 2000ms), then increments read_ref. MUST return a timeout error if the condition is not met within the deadline.
- **FR-007**: System MUST provide `take_write(key)` that waits until both read_ref=0 and write_ref=0 (using a hardcoded default timeout of 2000ms), then increments write_ref. MUST return a timeout error if the condition is not met within the deadline.
- **FR-008**: System MUST provide `release_read(key)` that atomically decrements read_ref. MUST return an error if read_ref is already 0.
- **FR-009**: System MUST provide `release_write(key)` that atomically decrements write_ref. MUST return an error if write_ref is already 0.
- **FR-010**: System MUST provide `downgrade_reference(key)` that atomically transitions from a write reference to a read reference (write_ref decremented and read_ref incremented in a single atomic step). MUST return an error if no write reference is held.
- **FR-011**: System MUST provide `remove(key)` that deletes the entry from the map. The call MUST return an error if any read or write references are still active; the caller is responsible for draining all references before removal.
- **FR-012**: On initialization, the `IEvictionPolicy` receptacle MUST be connected (returns an error if unbound). If an `IExtentManager` is also bound, the system MUST recover all committed extents by calling `IExtentManager::for_each_extent` and populating the map with their metadata. If no `IExtentManager` is bound, initialization MUST succeed with an empty map (returns `Ok(())`).
- **FR-013**: All `IDispatchMap` methods MUST be thread-safe and re-entrant, allowing concurrent calls from multiple threads.
- **FR-014**: System MUST use the `ILogger` receptacle for info, debug, and error logging throughout the component.
- **FR-015**: System MUST be implemented as a component using `define_component!` with `IDispatchMap` as a provided interface and `ILogger`, `IExtentManager`, and `IEvictionPolicy` as receptacles. The `IEvictionPolicy` receptacle is mandatory for initialization and provides LRU ordering for `touch()` and `oldest_keys()` operations.
- **FR-016**: System MUST provide `touch(key)` that marks the entry as most-recently-used via the `IEvictionPolicy` component (calling `ep.touch(handle)`) without acquiring any reference. MUST return `KeyNotFound` if the key does not exist. MUST NOT block or modify reference counts.
- **FR-017**: System MUST provide `oldest_keys(n)` that returns up to `n` keys ordered oldest-first by delegating to `IEvictionPolicy::get_eviction_candidates(pool, n)`. Used by the dispatcher's eviction logic to identify victim entries. MUST be thread-safe.
- **FR-018**: The dispatch map MUST support a `MemoryTier` location variant with fields: `pointer: *mut u8`, `size: u32`, `ssd_offset: Option<u64>`. System MUST provide `convert_memory_tier_to_block(key)` that transitions a MemoryTier entry to `BlockDevice` state — the offset is read from the entry's internal `ssd_offset` field (which must be `Some`) rather than passed as a parameter. Returns an error if the entry is not MemoryTier or has no `ssd_offset`.
- **FR-019**: *(Removed — DMA allocator injection is no longer needed; memory-tier entries accept externally-allocated pointers.)*
- **FR-020**: The `initialize()` method MUST be an explicit public API call (not implicitly called during construction). It rebuilds the map from extent manager state via `IExtentManager::for_each_extent` when an extent manager is bound. When no `IExtentManager` is bound, it returns `Ok(())` with an empty map.
- **FR-021**: *(Merged into FR-005 — `convert_to_storage` on a MemoryTier entry sets `ssd_offset` rather than transitioning to BlockDevice.)*
- **FR-022**: System MUST provide `is_evictable(key)` that returns `true` if and only if: the key exists in the map, the entry is in `MemoryTier` state with `ssd_offset: Some(_)` (write-through complete), and both `read_ref == 0` and `write_ref == 0` (no active references). Returns `false` for non-existent keys, non-MemoryTier entries, MemoryTier entries without `ssd_offset`, or entries with any active references.
- **FR-023**: System MUST provide `entry_size(key)` that returns the stored size of an entry in block-aligned bytes (`size_blocks * 4096`) without acquiring any reference. MUST return `KeyNotFound` if the key does not exist. Used by the dispatcher's `promote_to_memory_tier` to determine memory-tier allocation size for SSD-resident entries. Note: for memory-tier entries where the original size was not block-aligned, the returned value is rounded up to the nearest block boundary.
- **FR-024**: System MUST provide `recover_extent(key, size_blocks, offset)` that directly inserts a `BlockDevice` entry with the given offset and size (in 4KiB blocks), registers it with the eviction policy, and returns success. MUST return `AlreadyExists` if the key is already present. Used for incremental recovery (inserting individual extents) as an alternative to the bulk `initialize()` walk via `IExtentManager::for_each_extent`.
- **FR-025**: System MUST provide `promote_block_to_memory_tier(key, pointer, size)` that transitions a `BlockDevice` entry to `MemoryTier` **in place**, preserving the entry's `EvictionHandle` and ALL active reference counts (read_ref and write_ref are not reset or reinitialized). The entry's `MemoryTier.ssd_offset` MUST be set to `Some(original_offset)` so the promoted entry remains demotable back to `BlockDevice` without a fresh write-through. This is the inverse of `convert_memory_tier_to_block`, and — unlike `create_memory_tier_entry` followed by `remove` — it does not require the entry to be unreferenced first, so it works while the entry is pinned (`read_ref > 0`) during an in-flight load. MUST return `KeyNotFound` if the key is absent, `InvalidSize` if `size == 0`, and `InvalidState` if the entry is already in `MemoryTier` state. Consumed by the dispatcher's and dispatcher-p2p's on-demand cold-block promotion path on a read miss for SSD-resident data.
- **FR-026**: System MUST provide `try_evict_to_block(key)` that, under a single lock hold, atomically verifies the entry is in `MemoryTier` state with `ssd_offset: Some(_)` (write-through complete) and `read_ref == 0 && write_ref == 0` (no active references), then transitions it to `BlockDevice { offset }` using the recorded `ssd_offset`. This combines the `is_evictable` predicate (FR-022) and the `convert_memory_tier_to_block` transition (FR-018) into one atomic operation, eliminating the check-then-act race window that exists when calling them separately. MUST return `KeyNotFound` if the key is absent, or `InvalidState` if the entry is not evictable (active references held, no `ssd_offset` recorded, or the entry is not in `MemoryTier` state) — with no partial state change on error. After a successful call, no new reader can obtain the memory-tier pointer, so the caller may safely free the associated DRAM slot. Consumed by the dispatcher's and dispatcher-p2p's SSD-evictor background paths.
- **FR-027**: System MUST provide `clear()` that atomically removes all entries from the map, unregistering each from the `IEvictionPolicy` component, and returns the number of entries removed as a `usize`. Used by the dispatcher during namespace teardown to drop the entire cache in one call rather than iterating `oldest_keys`/`remove`. MUST succeed even when entries hold active references (teardown assumes all callers have already quiesced).

### Key Entities

- **CacheKey**: A `u64` value uniquely identifying an extent in the dispatch map.
- **Dispatch Entry**: Holds the location (`Location` enum), size in 4KiB blocks, read reference count (`u32`), write reference count (`u32`), and an `EvictionHandle` (opaque handle into the `IEvictionPolicy` component for LRU ordering). Protected by `Mutex`/`Condvar`.
- **Location**: An enum with two variants: `BlockDevice { offset: u64 }` for committed data on SSD, and `MemoryTier { pointer: *mut u8, size: u32, ssd_offset: Option<u64> }` for DRAM-cached entries. Note: `size_blocks` is stored on the `DispatchEntry`, not within the `Location` variant.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All committed extents are recoverable from persistent storage on component initialization — 100% of extents reported by the extent manager appear in the map after startup.
- **SC-002**: Concurrent readers accessing the same key experience no data corruption and no deadlocks under sustained multi-threaded access.
- **SC-003**: Write-to-read downgrade completes atomically with no window where the entry is unprotected (neither read-locked nor write-locked).
- **SC-004**: Per-entry metadata is kept compact. The `DispatchEntry` struct size varies by `Location` variant (`BlockDevice` stores a `u64` offset; `MemoryTier` stores a pointer, size, and optional offset).
- **SC-005**: Lookup of a cached key completes without blocking when no writer is active.
- **SC-006**: All reference count operations (take_read, take_write, release_read, release_write, downgrade) maintain consistent counts under concurrent access — no reference leaks or underflows.

## Clarifications

### Session 2026-04-27

- Q: Is the entry lifecycle one-way (MemoryTier → BlockDevice) or can entries transition back? → A: One-way. MemoryTier → BlockDevice only. To return data to the memory tier, the caller must remove and re-create.
- Q: What happens when `remove()` is called on an entry with active read or write references? → A: Returns an error. Caller must drain all references before calling remove.
- Q: What is the error behavior for invalid reference operations (underflow, no-write downgrade, size=0)? → A: Return error for all invalid operations. No panics or silent no-ops.
- Q: Should `take_read`/`take_write` block indefinitely or support a timeout? → A: Hardcoded default timeout of 2000ms (`DEFAULT_TIMEOUT`). Methods do not accept a per-call timeout parameter; the constant is used for all blocking operations.

### Session 2026-07-22 (spec-sync backfill)

- Q: The 2026-04-27 clarification states the `MemoryTier → BlockDevice` transition is one-way and that returning data to the memory tier requires remove-and-recreate. Does that still hold now that `promote_block_to_memory_tier` exists? → A: No — that clarification is superseded. `promote_block_to_memory_tier` (FR-025) provides an explicit, supported in-place `BlockDevice → MemoryTier` transition that preserves the eviction handle and all active references, specifically so a caller does not need to remove and re-create the entry while it may be pinned by an in-flight load. The full lifecycle is therefore bidirectional: `MemoryTier ⇄ BlockDevice` via `convert_memory_tier_to_block`/`try_evict_to_block` (MemoryTier→BlockDevice) and `promote_block_to_memory_tier` (BlockDevice→MemoryTier).

## Assumptions

- The caller is responsible for performing actual I/O to/from the memory-tier buffer and block device; the dispatch map only tracks metadata and locations.
- The `IExtentManager` receptacle is optional. When bound and initialized before the dispatch map's `initialize()` call, extent recovery populates the map from persisted state. When not bound, the dispatch map starts with an empty map.
- The `ILogger` receptacle is bound before any logging calls are made.
- Memory-tier buffer allocation is managed externally by the caller; the dispatch map accepts pre-allocated pointers via `create_memory_tier_entry`.
- A single dispatch map instance serves one storage namespace; multi-namespace support is out of scope for v0.
- `convert_to_storage` takes only `(key, offset)` — no `block_device_id` parameter. The component does not track which block device holds an extent.
- The `MemoryTier` location variant supports a DRAM caching tier where data resides in host memory before being written through to SSD.
- `convert_to_storage` on a MemoryTier entry sets the `ssd_offset` field rather than transitioning to BlockDevice; `convert_memory_tier_to_block` is the explicit transition method.
- The read_ref decrement in `convert_to_storage` is conditional — it only decrements if the current read_ref > 0, preventing underflow when no read reference is held at conversion time.
- The caller of `promote_block_to_memory_tier` is responsible for allocating the DRAM buffer and copying/staging the extent's data into it before (or as part of) the call; the dispatch map only updates the entry's location metadata.
- `promote_block_to_memory_tier` and `try_evict_to_block` are consumed by the `dispatcher` and `dispatcher-p2p` components as the read-miss promotion path and the SSD-evictor demotion path, respectively; the dispatch map itself has no scheduling or policy logic for when to promote or evict — that decision is made by the caller.
