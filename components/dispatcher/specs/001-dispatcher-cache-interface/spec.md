# Feature Specification: Dispatcher Cache Interface (Memory-Tier Architecture)

**Feature Branch**: `001-dispatcher-cache-interface`  
**Created**: 2026-05-08  
**Status**: Complete  
**Input**: Dispatcher component providing IDispatcher interface with DRAM memory-tier caching and write-through to SSD for GPU data flows

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Cache Population (GPU to Memory-Tier with Write-Through) (Priority: P1)

A client application holds data in GPU memory and wants to cache it for future use. The client calls the dispatcher's populate method, providing a cache key and an IPC handle referencing the GPU memory region. The dispatcher evicts entries from the memory-tier if capacity is insufficient, allocates a slot in the DRAM memory-tier via `IMemoryTier::insert()`, initiates a DMA copy from GPU memory into the memory-tier slot, registers the entry in the dispatch map as a `MemoryTier` entry (via `create_memory_tier_entry`), and returns confirmation to the client. In the background, the dispatcher reads from the memory-tier pointer via `IMemoryTier::peek()` (which does not refresh LRU order, ensuring the write-through does not prevent eviction of the entry) and asynchronously writes the data to the SSD via the block device and extent manager (write-through).

**Why this priority**: This is the primary write path — without the ability to populate the cache, no data enters the system. Every other operation depends on cached data existing.

**Independent Test**: Can be fully tested by populating a cache entry with a known key and verifying that the dispatch map contains a MemoryTier entry and that the data eventually reaches the block device (write-through completes). Delivers the core caching capability.

**Acceptance Scenarios**:

1. **Given** the dispatcher is initialized with all receptacles bound (including IMemoryTier), **When** populate(key, ipc_handle) is called with a new key, **Then** a memory-tier slot is allocated, DMA copy from GPU is performed into the slot, the entry is registered in the dispatch map as MemoryTier, and the call returns success before the SSD write-through completes.
2. **Given** a populate call returned successfully, **When** the background write-through completes, **Then** the dispatch map entry has its `ssd_offset` set (via `convert_to_storage`) and the entry remains in the memory-tier for fast future access.
3. **Given** the dispatcher is initialized, **When** populate(key, ipc_handle) is called with a key that already exists in the memory-tier, **Then** an `AlreadyExists` error is returned.
4. **Given** the memory-tier pool is at capacity, **When** populate(key, ipc_handle) is called, **Then** `evict_for_space` evicts LRU entries (transitioning them to BlockDevice state) until enough space is available, then the populate proceeds normally.

---

### User Story 2 - Cache Lookup with DMA Transfer (Priority: P1)

A client application needs to retrieve previously cached data into GPU memory. The client calls the dispatcher's lookup method, providing the cache key and an IPC handle for the destination GPU memory. The dispatcher queries the dispatch map:

- **MemoryTier hit**: DMA-copies directly from the memory-tier pointer to the client's GPU memory. This is the fast path.
- **BlockDevice (evicted from memory-tier)**: Promotes the entry back to the memory-tier using a pipelined SSD-to-DRAM-to-GPU reader, then re-registers it as a MemoryTier entry.
- **Staging (legacy)**: DMA-copies from the staging buffer to GPU. This path exists for backward compatibility but should not occur in normal memory-tier operation.

**Why this priority**: This is the primary read path. The cache is only useful if data can be retrieved. Lookup is the most latency-sensitive operation.

**Independent Test**: Can be tested by first populating a cache entry, then looking it up and verifying the DMA transfer to the client's memory occurs with correct data.

**Acceptance Scenarios**:

1. **Given** a cache entry exists in MemoryTier state, **When** lookup(key, ipc_handle) is called, **Then** a DMA copy from the memory-tier pointer to the GPU memory region is performed, the memory-tier LRU is touched, and success is returned.
2. **Given** a cache entry exists in BlockDevice state (evicted from memory-tier but on SSD), **When** lookup(key, ipc_handle) is called, **Then** the pipelined reader reads data from SSD into a new memory-tier slot while streaming chunks to GPU, the entry is re-registered as MemoryTier in the dispatch map, and success is returned.
3. **Given** no cache entry exists for the key, **When** lookup(key, ipc_handle) is called, **Then** a `KeyNotFound` error is returned.
4. **Given** a cache entry exists but with size mismatch, **When** lookup(key, ipc_handle) is called, **Then** an `InvalidParameter` error is returned.

---

### User Story 3 - Cache Presence Check (Priority: P2)

A client application wants to check whether a cache entry exists without transferring any data. The client calls the dispatcher's check method with a cache key. The dispatcher queries the dispatch map and returns whether the key is present (in any state: MemoryTier, BlockDevice, or Staging).

**Why this priority**: Enables clients to make decisions about whether to populate or look up data without incurring DMA transfer costs. Important for efficiency but not required for basic functionality.

**Independent Test**: Can be tested by checking a non-existent key (returns not present), populating a key, then checking again (returns present).

**Acceptance Scenarios**:

1. **Given** a cache entry exists for the key (in any state), **When** check(key) is called, **Then** the result indicates the entry is present.
2. **Given** no cache entry exists for the key, **When** check(key) is called, **Then** the result indicates the entry is not present.

---

### User Story 4 - Cache Entry Removal (Priority: P2)

A client application wants to evict a cache entry. The client calls the dispatcher's remove method with a cache key. The dispatcher removes the entry from the memory-tier (if present), removes the dispatch map entry, and frees the SSD extent (if the entry was in BlockDevice state).

**Why this priority**: Cache eviction is necessary for cache management and preventing resource exhaustion. Required for long-running workloads but not for basic single-use caching.

**Independent Test**: Can be tested by populating an entry, removing it, then verifying the key is no longer present and resources have been freed.

**Acceptance Scenarios**:

1. **Given** a cache entry exists in MemoryTier state, **When** remove(key) is called, **Then** the memory-tier slot is freed, the dispatch map entry is removed, and success is returned.
2. **Given** a cache entry exists in BlockDevice state, **When** remove(key) is called, **Then** the extent is freed via the extent manager, the dispatch map entry is removed, and success is returned.
3. **Given** no cache entry exists for the key, **When** remove(key) is called, **Then** a `KeyNotFound` error is returned.

---

### User Story 5 - Dispatcher Initialization and Wiring (Priority: P1)

A system integrator wires the dispatcher component to its dependencies: a logger, dispatch map, GPU services, SPDK environment, and memory-tier. The integrator provides PCI BDF address strings for data block devices via `DispatcherConfig`. The dispatcher internally creates N block device components and N extent managers, wiring them to the shared SPDK environment and logger. After initialization, the dispatcher starts the background write-through worker and is ready to serve cache operations. If the SPDK environment receptacle is not connected, the dispatcher operates in memory-tier-only mode (no write-through to SSD).

**Why this priority**: Without correct initialization and wiring, no cache operations can proceed. This is the prerequisite for all other stories.

**Independent Test**: Can be tested by wiring all receptacles and calling initialize, verifying that the dispatcher transitions to an operational state and that extent managers are correctly configured.

**Acceptance Scenarios**:

1. **Given** the dispatcher component is created, **When** logger, dispatch_map, gpu_services, spdk_env, and memory_tier receptacles are bound, **Then** initialize succeeds, N block devices and N extent managers are created internally, the background writer is started, and the dispatcher is ready for cache operations.
2. **Given** the dispatcher component is created, **When** initialize is called without the dispatch_map or memory_tier receptacle bound, **Then** an error is returned indicating the missing dependency.
3. **Given** initialization succeeds, **When** shutdown is called, **Then** the background writer drains all pending write-through jobs, block devices are shut down in reverse order, and resources are released.
4. **Given** the spdk_env receptacle is not connected but `data_pci_addrs` is non-empty in the config, **When** initialize is called, **Then** the dispatcher operates in memory-tier-only mode (no block devices created, no SSD persistence, write-through assigns synthetic offsets). Note: `data_pci_addrs` must always be provided (non-empty) regardless of whether spdk_env is connected; `initialize()` rejects an empty `data_pci_addrs` with `InvalidParameter` before evaluating the spdk_env connection state.

---

### User Story 6 - Direct Store Workflow (prepare/commit/cancel) (Priority: P2)

A caller wants to write data directly to SSD without going through the GPU DMA memory-tier path. The caller calls `prepare_store(key, size)` which reserves an SSD extent and returns a DMA buffer. The caller writes data into the buffer, then calls `commit_store(key)` to write the buffer to SSD and publish the extent, or `cancel_store(key)` to abort.

**Why this priority**: Enables alternative ingestion paths (e.g., host-to-SSD) that bypass the GPU DMA requirement, broadening the use cases for the cache.

**Independent Test**: Can be tested by calling prepare_store, writing data into the returned buffer, calling commit_store, and verifying the entry is accessible via check/lookup.

**Acceptance Scenarios**:

1. **Given** the dispatcher is initialized, **When** `prepare_store(key, size)` is called with a new key, **Then** the key is registered in the dispatch map, an extent is reserved on the target drive, and a DMA buffer of at least `size` bytes (block-aligned) is returned. The key is visible via `check()`.
2. **Given** a pending write exists for key, **When** `commit_store(key)` is called, **Then** the buffer contents are written to SSD using MDTS-aware segmented I/O, the extent is published, the dispatch map entry transitions to block-device state, and the write reference is released.
3. **Given** a pending write exists for key, **When** `cancel_store(key)` is called, **Then** the extent reservation is aborted (WriteHandle dropped), the dispatch map entry is removed, and no SSD write occurs.
4. **Given** `prepare_store` is called with a key that already exists, **Then** `AlreadyExists` error is returned.
5. **Given** `commit_store` or `cancel_store` is called with a key that has no pending write, **Then** `KeyNotFound` error is returned.

---

### User Story 7 - Memory-Tier Capacity Eviction (Priority: P2)

When the memory-tier pool does not have enough space for a new entry, the dispatcher must evict old entries to make room. Eviction is triggered by `populate` and `promote_and_serve` (the promotion path). The `evict_for_space` function uses a two-phase approach: it first queries `IMemoryTier::oldest_keys(MAX_SCAN)` to get candidate keys in LRU order, then checks each via `IDispatchMap::is_evictable(key)` to find entries whose write-through is complete (ssd_offset set) and have no active references. Evictable entries are removed from the memory-tier via `IMemoryTier::remove(key)` and transitioned to BlockDevice state via `convert_memory_tier_to_block`. If no evictable candidate is found, the function falls back to blind LRU eviction via `IMemoryTier::evict_lru()`, removing the dispatch-map entry entirely if the transition to BlockDevice fails (data loss accepted under extreme memory pressure). This loop continues until `used + needed <= capacity`.

**Why this priority**: Without eviction, the cache fills up and no new entries can be stored. Required for long-running workloads.

**Independent Test**: Can be tested by configuring a small memory-tier pool, populating past capacity, and verifying that old entries are evicted and transitioned to BlockDevice state.

**Acceptance Scenarios**:

1. **Given** memory-tier pool has 16 KiB capacity and 4 x 4 KiB entries exist, **When** a new 4 KiB entry is populated, **Then** `evict_for_space` queries `oldest_keys(128)` and selects entries where `dm.is_evictable(key)` returns true, removes them from the memory-tier via `mt.remove(key)`, and converts their dispatch-map records to BlockDevice until `used + 4096 <= 16384`.
2. **Given** entries that have completed write-through (ssd_offset set) and have no active references, **When** eviction runs, **Then** `is_evictable` returns true for those entries, they are preferred for eviction, and can be re-read from SSD later.
3. **Given** no entries pass the `is_evictable` check (all have in-flight write-through or active references), **When** eviction runs, **Then** `evict_for_space` falls back to blind LRU eviction via `evict_lru()`. If the evicted entry's `convert_memory_tier_to_block` fails, the dispatch-map entry is removed entirely (data loss accepted under extreme pressure).
4. **Given** the memory-tier has sufficient space for the requested allocation, **When** `evict_for_space` is called, **Then** no eviction occurs.

---

### User Story 8 - Touch (Refresh Eviction Priority) (Priority: P3)

A client wants to indicate that a cache entry is still in use without performing any data transfer. The client calls `touch(key)` to refresh the entry's eviction timestamp via the dispatch map, preventing it from being selected as a victim.

**Why this priority**: Touch enables efficient LRU-style eviction policies without the overhead of a full lookup (which involves DMA).

**Independent Test**: Can be tested by populating entries, touching one, triggering eviction, and verifying the touched entry survives.

**Acceptance Scenarios**:

1. **Given** a cache entry exists for the key, **When** `touch(key)` is called, **Then** the entry's timestamp is refreshed in the dispatch map and the call returns success. No DMA or memory-tier operations occur.
2. **Given** no cache entry exists for the key, **When** `touch(key)` is called, **Then** `KeyNotFound` error is returned.

---

### User Story 9 - Pipelined Promotion (SSD to Memory-Tier to GPU) (Priority: P2)

When a lookup hits an entry in BlockDevice state (evicted from memory-tier but persisted on SSD), the dispatcher promotes it back to the memory-tier and serves it to the GPU. The `promote_and_serve` function: (1) evicts memory-tier entries if needed, (2) allocates a new memory-tier slot, (3) uses the pipelined ring-buffer reader (`pipeline::pipelined_ssd_to_gpu`) to read from SSD in MDTS-sized chunks -- each chunk is copied to both the memory-tier slot and streamed to the GPU destination, (4) re-registers the entry as MemoryTier in the dispatch map with the original ssd_offset preserved.

**Why this priority**: Essential for handling the working-set churn case where frequently accessed entries get re-promoted after eviction. Without this, evicted entries cannot be served.

**Independent Test**: Can be tested by populating an entry, manually evicting it to BlockDevice state, then calling lookup and verifying the data arrives at the GPU and the entry is back in MemoryTier state.

**Acceptance Scenarios**:

1. **Given** an entry is in BlockDevice state at a known SSD offset, **When** lookup is called for that key, **Then** the pipelined reader reads from SSD directly into a new memory-tier slot (zero-copy path) or via intermediate ring buffers (fallback), streaming each chunk to the GPU via async DMA, and the entry is re-registered as MemoryTier.
2. **Given** the memory-tier is full when promotion is attempted, **When** lookup triggers promote_and_serve, **Then** `evict_for_space` is called first to free room before the new memory-tier slot is allocated.
3. **Given** no hardware is available (memory-tier-only mode), **When** promote_and_serve runs, **Then** it creates a memory-tier slot with zero-copied data, performs a direct GPU DMA from the slot, and re-registers the entry.

---

### User Story 10 - SSD Capacity Eviction (Priority: P3)

When the SSD data drives approach capacity, a background evictor removes the oldest (LRU by TSC timestamp) BlockDevice entries to prevent extent allocation failures during write-through. The evictor periodically checks combined SSD utilization (`used_bytes() / capacity_bytes()`) across all data drives. When utilization exceeds the high-water mark (`ssd_eviction_threshold`), it evicts entries in batches until utilization drops below the low-water mark (`ssd_eviction_low_watermark`). Entries in MemoryTier state are skipped (still hot in DRAM). Entries with active read/write references are skipped.

**Why this priority**: Without SSD eviction, drives fill up and all background write-throughs fail silently. New entries remain in memory-tier without SSD backing and are permanently lost on memory-tier eviction. This is critical for long-running workloads but operates transparently without client interaction.

**Independent Test**: Can be tested by configuring a low SSD eviction threshold, populating entries past the threshold, and verifying that old entries are removed from the dispatch map and their extents freed.

**Acceptance Scenarios**:

1. **Given** combined SSD utilization exceeds `ssd_eviction_threshold` (default 0.9), **When** the evictor wakes, **Then** it calls `oldest_keys(batch_size)` and evicts BlockDevice entries until utilization drops below `ssd_eviction_low_watermark` (default 0.8) or the batch is exhausted.
2. **Given** an entry is in MemoryTier state (still hot in DRAM), **When** the evictor evaluates it, **Then** it is skipped.
3. **Given** an entry has active read or write references, **When** the evictor attempts removal, **Then** `dm.remove()` fails and the entry is skipped without error.
4. **Given** `ssd_eviction_threshold` is set to 0.0 in DispatcherConfig, **When** the dispatcher initializes, **Then** the background evictor is NOT started.
5. **Given** shutdown is called while the evictor is running, **When** the evictor is mid-sweep, **Then** it finishes the current entry, exits the loop, and the thread joins cleanly.

---

### Edge Cases

- When memory-tier insertion fails during populate (pool full after eviction attempt), populate returns an `AllocationFailed` error to the caller and no dispatch map entry is created.
- When a populate is in progress (write reference held) and a lookup is called for the same key, the lookup can proceed if the write reference has been downgraded to a read reference (which happens immediately after the DMA copy completes and before background write-through begins).
- When the SSD is full and a background write-through cannot allocate an extent, the write-through silently fails (the entry remains in memory-tier but without SSD backing, making it vulnerable to permanent loss on eviction).
- When remove is called for a key, the memory-tier slot is freed first, then the dispatch-map entry is removed, then the SSD extent (if any) is freed.
- Multiple concurrent lookups for the same key are permitted (multiple read references allowed by dispatch map locking semantics).
- When the block device reports an I/O error during background write-through, the write-through is abandoned (the entry remains in memory-tier without SSD backing).
- When `prepare_store` fails after registering in the dispatch map (e.g., extent allocation failure), the dispatch map entry is cleaned up before returning the error.
- When eviction is triggered but no entries pass the `is_evictable` check (all have in-flight write-through or active references), eviction falls back to blind LRU via `evict_lru()`. If the evicted entry cannot transition to BlockDevice, its dispatch-map entry is removed entirely (data loss). This is an acceptable trade-off for preventing complete stalls.
- When the SSD evictor runs but all candidate entries have active references, no entries are evicted in that sweep. The evictor re-checks on its next interval.
- When the SSD evictor removes an entry, it frees the extent via the extent manager and removes the dispatch-map entry. No memory-tier operation is needed (the entry was already evicted from DRAM).

## Clarifications

### Session 2026-05-22 (Eviction Refinement & Background Writer)

- Q: How does `evict_for_space` choose which entry to evict? -> A: Two-phase approach. Phase 1: queries `IMemoryTier::oldest_keys(128)` for LRU candidates, then checks `IDispatchMap::is_evictable(key)` on each. `is_evictable` returns true when the entry is in MemoryTier state with `ssd_offset: Some(_)` (write-through complete) and no active read/write references. The first such entry is removed from the memory-tier via `mt.remove(key)` and transitioned via `convert_memory_tier_to_block`. Phase 2 (fallback): if no evictable candidate exists, calls `mt.evict_lru()` blindly and removes the dispatch-map entry if BlockDevice transition fails.
- Q: Why does the background writer use `peek()` instead of `get()`? -> A: `IMemoryTier::peek()` returns the pointer and size without refreshing the entry's LRU position. This ensures background write-through does not artificially keep entries "hot" and prevent their eviction under memory pressure.
- Q: What is `IMemoryTier::oldest_keys(n)`? -> A: Returns up to N keys in LRU order (oldest first) without removing them. Used by `evict_for_space` to scan for eviction candidates.
- Q: What is `IDispatchMap::is_evictable(key)`? -> A: Returns true if the entry exists in MemoryTier state with a non-None `ssd_offset` and has no active read or write references. Used by `evict_for_space` to identify safe eviction candidates.

### Session 2026-05-12 (SSD Eviction)

- Q: What about `max_cache_entries` and `eviction_threshold` in DispatcherConfig? -> A: These are vestigial from the v0 count-based eviction and are unused in v1. Memory-tier eviction is purely capacity-based (FR-024). They are retained in the struct for API backward compatibility but should be considered deprecated.
- Q: How does the SSD evictor determine drive ownership for extent removal? -> A: Uses `key % num_drives` to identify the target drive, matching the write-through path's drive selection.

### Session 2026-05-08 (Memory-Tier Rewrite)

- Q: How is the memory-tier pool managed? -> A: The `IMemoryTier` interface provides `insert()`, `get()`, `peek()`, `remove()`, `evict_lru()`, `oldest_keys()`, `touch()`, `capacity()`, and `used()`. The `peek()` method returns the pointer and size without refreshing LRU (used by the background writer). The `oldest_keys(n)` method returns up to N keys in LRU order without removing them (used by `evict_for_space`). The pool is pre-allocated DRAM. Eviction is capacity-based: `used + needed > capacity` triggers LRU eviction.
- Q: What happens when an evicted entry is looked up? -> A: The dispatch-map returns `LookupResult::BlockDevice { offset }`. The dispatcher calls `promote_and_serve` which reads from SSD, inserts into memory-tier, DMA-copies to GPU, and re-registers in the dispatch-map.
- Q: What happens if write-through hasn't completed when eviction occurs? -> A: The two-phase eviction preferentially selects entries where `is_evictable()` is true (write-through complete, no active references). Entries with incomplete write-through are only evicted as a last resort (fallback to blind `evict_lru`). In the fallback case, `convert_memory_tier_to_block` fails (no ssd_offset set), the dispatch-map entry is removed entirely, and the data is effectively lost. This is accepted as unlikely in practice due to the preference for evictable entries.
- Q: Does lookup update the LRU order? -> A: Yes, `mt.touch(key)` is called after a successful MemoryTier lookup to refresh the entry's position in the LRU.
- Q: How does the pipelined reader work? -> A: The primary path (`pipelined_ssd_to_gpu_zero_copy`) reads NVMe directly into the memory-tier slot (CUDA-pinned + SPDK-registered), then issues `cudaMemcpyAsync` H2D from the same memory on alternating CUDA streams, with up to 16 NVMe reads in flight. The fallback path (`pipelined_ssd_to_gpu`) allocates a ring of 8 DMA buffers and copies each chunk from ring → memory-tier slot + GPU.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST define an `IDispatcher` interface in the shared interfaces crate, providing `initialize`, `shutdown`, `lookup`, `lookup_async`, `check`, `remove`, `populate`, `prepare_store`, `commit_store`, `cancel_store`, and `touch` methods.
- **FR-002**: System MUST define a `DispatcherError` error type in the shared interfaces crate, covering all failure modes (not initialized, key not found, duplicate key, I/O error, allocation failure, invalid parameter).
- **FR-003**: The `populate(key, ipc_handle)` method MUST allocate a slot in the DRAM memory-tier via `IMemoryTier::insert()`, initiate DMA copy from the client's GPU memory into the memory-tier slot via `IGpuServices::dma_copy_to_host`, register the entry in the dispatch map as a MemoryTier entry (via `create_memory_tier_entry`), and enqueue an asynchronous background write-through job. MUST run `evict_for_space` if the memory-tier pool lacks capacity.
- **FR-004**: After a successful populate, the system MUST asynchronously read from the memory-tier pointer (via `IMemoryTier::peek()` to avoid refreshing LRU order) and write the data to the SSD via the block device and extent manager (write-through), calling `convert_to_storage` on completion to record the SSD offset.
- **FR-005**: The memory-tier entry MUST remain accessible for fast lookups even after the background SSD write-through completes. The memory-tier slot is NOT freed on write-through completion.
- **FR-006**: The `lookup(key, ipc_handle)` method MUST query the dispatch map: if MemoryTier, DMA from memory-tier pointer to GPU and touch LRU; if BlockDevice (evicted), promote back to memory-tier via pipelined SSD-to-DRAM-to-GPU reader and re-register as MemoryTier; if Staging (legacy), DMA from staging buffer to GPU. When the memory-tier pool is CUDA-pinned (registered via FR-035), the MemoryTier path SHOULD use `IGpuServices::dma_copy_to_device_async` with a CUDA stream followed by `stream_synchronize` for the H2D transfer. Falls back to synchronous `dma_copy_to_device` when no CUDA streams are available.
- **FR-007**: The `lookup` method MUST return a cache-miss indication if the key does not exist in the dispatch map.
- **FR-008**: The `check(key)` method MUST return whether a cache entry exists for the given key without performing any data transfer.
- **FR-009**: The `remove(key)` method MUST free the memory-tier slot (if present via `mt.remove()`), free the extent on SSD (if in BlockDevice state), and remove the dispatch map entry.
- **FR-010**: The dispatcher component MUST use the component framework's `define_component!` macro and expose only the `IDispatcher` interface.
- **FR-011**: The dispatcher MUST accept receptacles for `ILogger`, `IDispatchMap`, `IGpuServices`, `ISPDKEnv`, and `IMemoryTier` components. Block devices and extent managers are created internally during initialization.
- **FR-012**: The `initialize` method MUST validate that the `dispatch_map` and `memory_tier` receptacles are bound before proceeding. Other receptacles are validated lazily on first use.
- **FR-013**: The dispatcher MUST use appropriate read/write locking on the dispatch map to ensure thread safety during concurrent operations.
- **FR-014**: The `shutdown` method MUST ensure all in-flight background operations complete or are cancelled before returning.
- **FR-015**: The dispatcher MUST coordinate N data block devices with N extent managers, where each extent manager is associated with a specific metadata partition and data block device.
- **FR-016**: The dispatcher MUST pass the data block device size and computed FormatParams to each extent manager's format function.
- **FR-017**: When the asynchronous background write-through fails (extent allocation failure or block device I/O error), the background writer silently drops the job. The dispatch map entry remains in MemoryTier state. (Known limitation.)
- **FR-018**: The `remove(key)` method does NOT block waiting for background write-through to complete. It proceeds immediately with removal.
- **FR-019**: All block device I/O operations MUST be segmented to respect MDTS (typically 128 KiB). The primary promotion path uses a zero-copy pipeline: NVMe reads directly into the memory-tier slot (which is CUDA-pinned + SPDK-registered via FR-035), then issues async H2D DMA from the same memory to GPU, with pipeline depth up to 16 concurrent NVMe reads. A ring-buffer fallback path (`pipelined_ssd_to_gpu`) exists for when the memory-tier pool is not registered for DMA.
- **FR-020**: The `prepare_store(key, size)` method MUST run eviction if the cache is over capacity, reserve an extent on the target data drive, register the key in the dispatch map, and return a DMA buffer for the caller to write into. MUST return `AlreadyExists` if the key exists, `AllocationFailed` if extent reservation fails, `InvalidParameter` if size is 0.
- **FR-021**: The `commit_store(key)` method MUST write the pending DMA buffer contents to SSD using MDTS-aware segmented I/O, publish the extent metadata, and transition the dispatch map entry to block-device state. MUST return `KeyNotFound` if no pending write exists.
- **FR-022**: The `cancel_store(key)` method MUST drop the pending write (aborting the extent reservation via WriteHandle destructor) and remove the dispatch map entry. MUST return `KeyNotFound` if no pending write exists.
- **FR-023**: The `touch(key)` method MUST update the entry's eviction timestamp in the dispatch map without performing any DMA transfer or acquiring any reference. MUST return `KeyNotFound` if the key does not exist.
- **FR-024**: Eviction in v1 is purely capacity-based within the memory-tier pool. When the pool is full, `evict_for_space` uses a two-phase approach: (1) queries `IMemoryTier::oldest_keys(MAX_SCAN)` for LRU candidates and selects entries where `IDispatchMap::is_evictable(key)` returns true (write-through complete, no active references), removing them via `IMemoryTier::remove(key)` and transitioning via `convert_memory_tier_to_block`; (2) if no evictable candidate exists, falls back to blind `IMemoryTier::evict_lru()` and removes the dispatch-map entry if the transition fails. Count-based TSC eviction (from v0) is NOT used in v1.
- **FR-025**: The `DispatcherConfig` MUST support a `format_on_init` flag (default true). When false, extent managers are not reformatted on initialization, preserving on-disk data from previous sessions.
- **FR-026**: ~~REMOVED~~ *(superseded 2026-05-21)* — Block device version selection is no longer required. The implementation hardcodes a single block device component; there is no version multiplexing.
- **FR-027**: ~~REMOVED~~ *(superseded 2026-05-21)* — Extent manager version selection is no longer required. The implementation hardcodes a single extent manager; there is no version multiplexing.
- **FR-028**: On BlockDevice lookup (promotion), the pipelined reader MUST re-insert the entry into the memory-tier and re-register it as a MemoryTier entry in the dispatch map.
- **FR-029**: The dispatcher MUST start a background SSD evictor thread during `initialize()` if `ssd_eviction_threshold > 0.0` and at least one data drive is configured. The evictor MUST be shut down (thread joined) during `shutdown()`.
- **FR-030**: The SSD evictor MUST periodically check combined SSD utilization (sum of `IExtentManager::used_bytes()` / sum of `IExtentManager::capacity_bytes()` across all extent managers). The check interval MUST be configurable via `ssd_eviction_interval_secs` (default: 5 seconds).
- **FR-031**: When SSD utilization exceeds `ssd_eviction_threshold` (default: 0.9), the evictor MUST evict BlockDevice-only entries using `IDispatchMap::oldest_keys(batch_size)` for LRU ordering, stopping when utilization drops below `ssd_eviction_low_watermark` (default: 0.8) or the batch is exhausted.
- **FR-032**: The SSD evictor MUST skip entries in MemoryTier state (still hot in DRAM). Entries with active read or write references MUST be skipped gracefully (dm.remove fails without panic).
- **FR-033**: The `DispatcherConfig` MUST include `ssd_eviction_threshold` (f64, default 0.9), `ssd_eviction_low_watermark` (f64, default 0.8), `ssd_eviction_batch_size` (usize, default 64), and `ssd_eviction_interval_secs` (u64, default 5). Setting `ssd_eviction_threshold` to 0.0 disables the evictor.
- **FR-034**: During `initialize()`, after GPU and memory-tier are ready, the dispatcher MUST call `IGpuServices::register_host_memory` on the memory-tier pool to CUDA-pin and SPDK-register it for zero-copy NVMe and GPU DMA. Registration failure MUST be logged but MUST NOT be fatal (the system falls back to the ring-buffer pipeline path). The dispatcher SHOULD also cache block-device `ClientChannels` per data drive at init time to avoid per-operation connection overhead.
- **FR-035**: During `shutdown()`, before memory-tier teardown, the dispatcher MUST call `IGpuServices::unregister_host_memory` to release CUDA page-locking and SPDK registration on the memory-tier pool.
- **FR-036**: System MUST provide a `lookup_async(key, ipc_handle) -> Result<GpuStream, DispatcherError>` method on the `IDispatcher` interface. This method performs the same cache lookup as `lookup` (dispatch-map query, memory-tier hit, BlockDevice promotion, staging fallback) but returns a `GpuStream` handle instead of blocking on the H2D DMA transfer. The caller MUST call `stream_synchronize` on the returned stream before accessing the GPU destination memory. For non-memory-tier paths (staging, SSD promotion) the copy completes synchronously and a null stream is returned. The synchronous `lookup` method MUST delegate to `lookup_async` internally and call `stream_synchronize` before returning.

### Key Entities

- **CacheKey**: A 64-bit identifier for cached data elements. Used to address entries in the dispatch map and memory-tier.
- **IPC Handle**: Contains a pointer to a GPU memory region and a size. Used for DMA transfers between GPU and host memory.
- **Memory-Tier Slot**: A region in the pre-allocated DRAM pool managed by `IMemoryTier`. Holds cached data for fast access. Eviction is capacity-based (bytes used vs pool size).
- **Dispatch Map Entry**: A record tracking the state of a cached element -- MemoryTier (pointer + size + optional ssd_offset), BlockDevice (ssd_offset only), or Staging (legacy DMA buffer).
- **Extent**: A contiguous region on a data block device, managed by the extent manager, used to store write-through cache data.
- **Data Block Device**: An NVMe SSD that holds persisted cache data. There are N data block devices in the system.
- **Background Writer**: A dedicated thread that processes write-through jobs, reading from memory-tier pointers via `IMemoryTier::peek()` (to avoid refreshing LRU) and writing to SSD via extent managers.
- **Pipelined Reader**: A zero-copy pipeline (`pipeline.rs`) that reads SSD data in MDTS-sized chunks directly into the memory-tier slot (which is CUDA-pinned + SPDK-registered), then issues async H2D GPU DMA from the same memory. Falls back to a ring-buffer approach (`pipelined_ssd_to_gpu`) when the memory-tier pool is not registered for DMA.
- **PendingWrite**: A temporary structure holding a WriteHandle (extent reservation), DMA buffer, size, and drive index for the prepare_store/commit_store/cancel_store workflow.
- **Background SSD Evictor**: A dedicated thread that periodically checks SSD utilization and evicts the oldest BlockDevice entries when capacity exceeds a threshold.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A client can populate a cache entry and subsequently retrieve it via lookup (memory-tier hit path), receiving the correct data, within a single session.
- **SC-002**: Cache check operations return accurate presence information for both existing and non-existing keys.
- **SC-003**: Cache removal frees all associated resources (memory-tier slots and SSD extents) so that they can be reused.
- **SC-004**: The dispatcher correctly handles concurrent populate and lookup operations on different keys without data corruption or deadlock.
- **SC-005**: Initialization fails gracefully with a descriptive error when required dependencies (dispatch_map, memory_tier) are not bound.
- **SC-006**: Shutdown drains all pending background write-through jobs before returning, ensuring data consistency.
- **SC-007**: The dispatcher supports N independent data block devices and extent managers operating in parallel, with key-based drive selection.
- **SC-008**: The prepare_store/commit_store workflow successfully persists data to SSD and makes it retrievable via lookup.
- **SC-009**: Capacity-based eviction correctly removes LRU entries from the memory-tier when the pool is full, transitioning them to BlockDevice state.
- **SC-010**: The touch operation refreshes an entry's dispatch-map timestamp without performing DMA.
- **SC-011**: Entries evicted from memory-tier but present on SSD can be promoted back via the pipelined reader on subsequent lookup.
- **SC-012**: The pipelined reader correctly streams MDTS-sized chunks from SSD to both memory-tier and GPU in a single pass.
- **SC-013**: The background SSD evictor removes the oldest BlockDevice entries when SSD utilization exceeds the configured threshold, freeing extents until utilization drops below the low-water mark.

## Assumptions

- Clients provide valid IPC handles referencing accessible GPU memory regions. The dispatcher does not validate GPU memory accessibility.
- The SPDK environment is initialized and active before the dispatcher's `initialize()` is called (via the ISPDKEnv receptacle). When ISPDKEnv is not connected, the dispatcher operates in memory-tier-only mode.
- The memory-tier pool is pre-allocated before initialization. The dispatcher does not manage the pool lifecycle (only calls insert/get/peek/remove/evict_lru/oldest_keys/touch).
- DMA buffer allocation for the pipelined reader and prepare_store uses SPDK DMA allocation (falls back to libc `aligned_alloc` without SPDK).
- GPU-to-host DMA transfers use `IGpuServices::dma_copy_to_host` (populate direction). Host-to-GPU DMA transfers use `IGpuServices::dma_copy_to_device_async` with stream synchronization when the pool is CUDA-pinned, falling back to synchronous `dma_copy_to_device` (lookup direction).
- NVMe SSDs have a Maximum Data Transfer Size (MDTS) limit. The `io_segmenter` module provides MDTS-aware I/O splitting.
- Memory-tier pointers are wrapped in DmaBuffer with a `noop_free` function -- the memory-tier component owns the memory; the DmaBuffer wrapper must not free it.
- Write-through is best-effort: failure does not propagate to the caller. Data remains accessible from the memory-tier until eviction.
- The background writer holds a read reference on each entry while write-through is in progress. The reference is released after write completes or fails.
- Block devices and extent managers are created internally during `initialize()` -- callers provide PCI BDF address strings, not pre-constructed components.
