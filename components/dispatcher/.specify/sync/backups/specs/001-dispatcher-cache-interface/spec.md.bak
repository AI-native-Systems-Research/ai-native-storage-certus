# Feature Specification: Dispatcher Cache Interface (Memory-Tier Architecture)

**Feature Branch**: `001-dispatcher-cache-interface`  
**Created**: 2026-05-08  
**Status**: Complete  
**Input**: Dispatcher component providing IDispatcher interface with DRAM memory-tier caching and write-through to SSD for GPU data flows  
**Last Synced**: 2026-08-07 — drift sweep on branch `sync/spec-drift-sweep-20260807`. All backfills (spec→code): FR-001 method inventory expanded to the full shipped `IDispatcher` surface; FR-039 `batch_lookup` signature corrected to multi-region `&[(CacheKey, Vec<IpcHandle>)]`; FR-042 `create_eviction_channel` given its `capacity: usize` argument; FR-033 config list extended with the partition-size and backfill-delay fields; new FR-056 documents the GPU-staged memory-lifecycle primitives (reserve/copy/complete/release/pin/unpin) and the flush/stats methods. Separately, the `IDispatcher` interface doc comment previously advertised a Creusot proof tree (`components/dispatcher/verif/`, "24 verification conditions discharged by SMT solvers") that does not exist; that claim was softened to "informal design invariants" in `interfaces/src/idispatcher.rs` on the same branch. No behavior changes.

**Last Synced**: 2026-08-20 — Spec-Sync Phase B. BACKFILL: User Story 11 narrative + acceptance scenario 3 corrected from the stale `16 / num_queues` (`max_queue_depth = 8`, ≤16 per drive) wording to `max_queue_depth = 128` per thread, resolving an intra-spec contradiction with FR-039 step (5) / FR-019 and matching `src/lib.rs:2217`. BACKFILL-UNSPECCED: new FR-057 + SC-016 document the `DispatcherComponent` dependency-injection / test surface (`set_block_device_factory`, `set_extent_manager_factory`, `set_pipeline_metrics`). Two documentation-only drifts in the component `CLAUDE.md` (stale `../../component-framework/crates/` path after the `components/ → lib/` move; stale `-v2` crate names) are recorded in `.specify/sync/proposals.*` as BACKFILL but were NOT applied here — `CLAUDE.md` is outside this sync's editable scope (`.specify/sync/` and `specs/` only) and is left for a follow-up doc pass. No source or behavior changes.

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

**Why this priority**: This is the primary read path. The cache is only useful if data can be retrieved. Lookup is the most latency-sensitive operation.

**Independent Test**: Can be tested by first populating a cache entry, then looking it up and verifying the DMA transfer to the client's memory occurs with correct data.

**Acceptance Scenarios**:

1. **Given** a cache entry exists in MemoryTier state, **When** lookup(key, ipc_handle) is called, **Then** a DMA copy from the memory-tier pointer to the GPU memory region is performed, the memory-tier LRU is touched, and success is returned.
2. **Given** a cache entry exists in BlockDevice state (evicted from memory-tier but on SSD), **When** lookup(key, ipc_handle) is called, **Then** the pipelined reader reads data from SSD into a new memory-tier slot while streaming chunks to GPU, the entry is re-registered as MemoryTier in the dispatch map, and success is returned.
3. **Given** no cache entry exists for the key, **When** lookup(key, ipc_handle) is called, **Then** a `KeyNotFound` error is returned.
4. **Given** a cache entry exists but with size mismatch, **When** lookup(key, ipc_handle) is called, **Then** an `InvalidParameter` error is returned.

---

### User Story 3 - Cache Presence Check (Priority: P2)

A client application wants to check whether a cache entry exists without transferring any data. The client calls the dispatcher's check method with a cache key. The dispatcher queries the dispatch map and returns whether the key is present (in any state: MemoryTier or BlockDevice).

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

### ~~User Story 6 - Direct Store Workflow~~ *(REMOVED 2026-07-02)*

*The prepare_store/commit_store/cancel_store direct-write workflow has been removed. Data ingestion uses the GPU DMA memory-tier path (User Story 1) exclusively.*

---

### User Story 7 - Memory-Tier Capacity Eviction (Priority: P2)

When the memory-tier pool does not have enough space for a new entry, the dispatcher must evict old entries to make room. Eviction is triggered by `populate` and `promote_and_serve` (the promotion path). The `evict_for_space` function MUST be pin-safe: each iteration frees one victim via `evict_one_clean`, which scans `IMemoryTier::oldest_keys(scan)` over a window that widens with sustained pressure (`MAX_SCAN=4 × attempts`, capped at 1024) to look past pinned or unpersisted entries deeper in the LRU. For each scanned candidate, in order of preference, it (1) demotes to BlockDevice via `IDispatchMap::try_evict_to_block` (requires write-through complete and unpinned) followed by `IMemoryTier::remove`, or (2) if unpinned but write-through is incomplete, drops it entirely via `dm.remove` + `mt.remove` (data loss — recomputed on next miss). Entries with active read/write references are never freed. This loop continues until `used + needed <= capacity` or `max_attempts` (default 2048, configurable via `DispatcherConfig::max_eviction_attempts`) iterations are exhausted, in which case it returns `AllocationFailed` rather than blind-freeing a pinned slot (see FR-024).

**Why this priority**: Without eviction, the cache fills up and no new entries can be stored. Required for long-running workloads.

**Independent Test**: Can be tested by configuring a small memory-tier pool, populating past capacity, and verifying that old entries are evicted and transitioned to BlockDevice state.

**Acceptance Scenarios**:

1. **Given** memory-tier pool has 16 KiB capacity and 4 x 4 KiB entries exist, **When** a new 4 KiB entry is populated, **Then** `evict_for_space` iterates, freeing one pin-safe victim per iteration via `evict_one_clean` over a widening `oldest_keys` scan, until `used + 4096 <= 16384`.
2. **Given** entries that have completed write-through (ssd_offset set) and have no active references, **When** `evict_one_clean` scans the LRU window, **Then** those entries are demoted to BlockDevice via `try_evict_to_block` + `mt.remove` and can be re-read from SSD later.
3. **Given** a scanned candidate is unpinned but has not completed write-through (no active references but no ssd_offset), **When** eviction runs, **Then** `evict_one_clean` drops it entirely via `dm.remove` + `mt.remove` (data loss accepted under extreme pressure) rather than demoting it. Candidates with active read/write references are always skipped and never freed.
4. **Given** the memory-tier has sufficient space for the requested allocation, **When** `evict_for_space` is called, **Then** no eviction occurs.
5. **Given** all memory-tier entries have active read/write references and cannot be evicted, **When** `evict_for_space` iterates `max_eviction_attempts` (default 2048) times without freeing enough space, **Then** it returns `AllocationFailed` and the caller (populate or promote_and_serve) propagates this error to the client.

---

### User Story 8 - Touch (Refresh Eviction Priority) (Priority: P3)

A client wants to indicate that a cache entry is still in use without performing any data transfer. The client calls `touch(key)` to refresh the entry's eviction timestamp in the dispatch map and its LRU position in the memory-tier (if resident), preventing it from being selected as a victim by either the SSD evictor or the memory-tier capacity evictor.

**Why this priority**: Touch enables efficient LRU-style eviction policies without the overhead of a full lookup (which involves DMA).

**Independent Test**: Can be tested by populating entries, touching one, triggering eviction, and verifying the touched entry survives.

**Acceptance Scenarios**:

1. **Given** a cache entry exists for the key, **When** `touch(key)` is called, **Then** the entry's timestamp is refreshed in the dispatch map and its memory-tier LRU position is updated (if resident in DRAM). No DMA operations occur.
2. **Given** no cache entry exists for the key, **When** `touch(key)` is called, **Then** `KeyNotFound` error is returned.

---

### User Story 9 - Pipelined Promotion (SSD to Memory-Tier to GPU) (Priority: P2)

When a lookup hits an entry in BlockDevice state (evicted from memory-tier but persisted on SSD), the dispatcher promotes it back to the memory-tier and serves it to the GPU. The `promote_and_serve` function: (1) evicts memory-tier entries if needed, (2) allocates a new memory-tier slot, (3) uses the sliding-window zero-copy pipeline (`pipeline::pipelined_ssd_to_gpu_zero_copy`) — maintains up to `max_queue_depth` concurrent in-flight NVMe reads; as each read completes the GPU H2D copy is issued immediately for that segment and the next NVMe read is submitted, overlapping SSD I/O with GPU DMA. Correctness relies on FIFO completion ordering within a single NVMe queue pair. (4) re-registers the entry as MemoryTier in the dispatch map with the original ssd_offset preserved.

**Why this priority**: Essential for handling the working-set churn case where frequently accessed entries get re-promoted after eviction. Without this, evicted entries cannot be served.

**Independent Test**: Can be tested by populating an entry, manually evicting it to BlockDevice state, then calling lookup and verifying the data arrives at the GPU and the entry is back in MemoryTier state.

**Acceptance Scenarios**:

1. **Given** an entry is in BlockDevice state at a known SSD offset, **When** lookup is called for that key, **Then** the pipelined reader uses a sliding window of up to `max_queue_depth=16` concurrent NVMe reads; each completed read immediately triggers a GPU H2D async copy and submits the next NVMe read, overlapping I/O stages. The entry is re-registered as MemoryTier. The fallback path (ring-buffer `pipelined_ssd_to_gpu`) is used when the memory-tier pool is not CUDA-pinned/SPDK-registered.
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

### User Story 11 - Parallel Batch Cold Promotion (Priority: P1)

A client application submits a batch of cache keys to `batch_lookup`. For entries in BlockDevice state (cold — evicted from memory-tier but on SSD), the dispatcher promotes them in parallel across drives and queue threads. Each data drive's cold entries are split across up to `MAX_QUEUES_PER_DRIVE` (default 2) threads, each with its own NVMe client channels and CUDA streams. Each thread drives its cold promotions with a deep per-thread NVMe pipeline depth (`max_queue_depth = 128`, per FR-039 step (5) and FR-019) to saturate the drive's submission queue for maximum per-drive parallelism. This delivers significantly higher single-client cold throughput by exploiting drive-level and queue-level parallelism.

**Why this priority**: Single-entry sequential cold promotion yields only ~0.34 GB/s per client (limited by queue-depth-1 per drive). Parallel batch promotion reaches ~5.6 GB/s by saturating multiple drives and queues concurrently, closing the gap between cold and hot throughput for inference workloads that experience working-set churn.

**Independent Test**: Can be tested by populating entries, clearing the memory-tier (forcing all to BlockDevice state), then calling `batch_lookup` with a batch of 20 keys and verifying all are served correctly with wall time significantly lower than 20 × sequential-lookup time.

**Acceptance Scenarios**:

1. **Given** 20 entries exist in BlockDevice state spread across 3 drives, **When** `batch_lookup` is called with all 20 keys and IPC handles, **Then** cold entries are promoted in parallel (per-drive thread groups), all results are returned in input order, and total latency is bounded by the slowest single drive (not sum of all entries).
2. **Given** a batch contains a mix of MemoryTier (hot) and BlockDevice (cold) entries, **When** `batch_lookup` is called, **Then** hot entries are served inline without waiting for cold promotions to complete, and cold entries are promoted in parallel.
3. **Given** `MAX_QUEUES_PER_DRIVE = 2`, **When** a drive has 10 cold entries, **Then** entries are split into two groups of 5, each processed by a separate thread, and each thread drives its cold promotions with `max_queue_depth = 128` (per FR-039 step (5) / FR-019) to saturate the drive's NVMe submission queue.
4. **Given** a cold entry's NVMe read or GPU DMA fails, **When** the thread encounters the error, **Then** the error is reported for that entry only; other entries in the batch continue to be processed independently.
5. **Given** entries that do not exist in the dispatch map, **When** `batch_lookup` is called, **Then** those entries receive `KeyNotFound` errors while valid entries are served normally.

---

### User Story 12 - Background Memory-Tier Demotion (Priority: P3)

When the memory-tier DRAM pool approaches capacity, a background evictor proactively demotes the oldest (LRU) entries from DRAM to SSD-backed BlockDevice state before on-demand eviction pressure causes stalls on the populate or cold-promotion hot paths. The evictor periodically checks memory-tier utilization (`used() / capacity()`). When utilization exceeds the configured threshold (`memory_tier_eviction_threshold`), it demotes entries in batches until utilization drops below the low-water mark (`memory_tier_eviction_low_watermark`). Only entries whose write-through is complete (`is_evictable`) are eligible for demotion. The demotion path is: `IMemoryTier::remove(key)` followed by `IDispatchMap::convert_memory_tier_to_block(key)` — the data remains accessible on SSD via the BlockDevice lookup path.

**Why this priority**: Proactive demotion prevents memory-tier exhaustion before it causes `AllocationFailed` errors on the populate hot path. Without this, the only memory-tier eviction is reactive (triggered during populate), which adds latency to client requests under sustained write load.

**Independent Test**: Can be tested by configuring a small memory-tier pool with threshold=0.5, populating past 50% capacity, and verifying that old entries are proactively transitioned to BlockDevice state within the evictor interval.

**Acceptance Scenarios**:

1. **Given** memory-tier utilization exceeds `memory_tier_eviction_threshold` (e.g. 0.8), **When** the evictor wakes, **Then** it scales the batch via quadratic pressure (1×–8× base), calls `oldest_keys(scan_size)` where scan_size widens on consecutive dry runs, and demotes entries via `try_evict_to_block` (atomic evictability check + transition) followed by `mt.remove` until utilization drops below `memory_tier_eviction_low_watermark` (default 0.70) or the scan is exhausted. Successful batches are paced at 200ms; dry runs back off 500ms.
2. **Given** an entry has not completed write-through or has active references, **When** `try_evict_to_block` is called, **Then** it returns an error and the entry is skipped (not removed from dispatch-map).
3. **Given** `memory_tier_eviction_threshold` is set to 0.0 in DispatcherConfig (default), **When** the dispatcher initializes, **Then** the background memory-tier evictor is NOT started.
4. **Given** shutdown is called while the memory-tier evictor is running, **When** the evictor is mid-sweep, **Then** it finishes the current entry, exits the loop, and the thread joins cleanly.
5. **Given** a successful demotion, **When** the entry's key is subsequently looked up, **Then** the entry is served via the BlockDevice cold path (SSD promotion back to DRAM).

---

### Edge Cases

- When memory-tier insertion fails during populate (pool full after eviction attempt), populate returns an `AllocationFailed` error to the caller and no dispatch map entry is created.
- When `evict_for_space` cannot free sufficient memory-tier space within `max_attempts` iterations (default 2048, configurable via `DispatcherConfig::max_eviction_attempts`; e.g. every scanned candidate is pinned or has incomplete write-through and cannot be freed), it returns `AllocationFailed` directly rather than blind-freeing a pinned slot. This propagates to the caller (populate or promote_and_serve), which leaves the block uncached or serves it via the staging pool (FR-053), identically to an `mt.insert()` failure.
- When a populate is in progress (write reference held) and a lookup is called for the same key, the lookup can proceed if the write reference has been downgraded to a read reference (which happens immediately after the DMA copy completes and before background write-through begins).
- When the SSD is full and a background write-through cannot allocate an extent, the write-through silently fails (the entry remains in memory-tier but without SSD backing, making it vulnerable to permanent loss on eviction).
- When remove is called for a key, the memory-tier slot is freed first, then the dispatch-map entry is removed, then the SSD extent (if any) is freed.
- Multiple concurrent lookups for the same key are permitted (multiple read references allowed by dispatch map locking semantics).
- When the block device reports an I/O error during background write-through, the write-through is abandoned (the entry remains in memory-tier without SSD backing).
- When eviction is triggered and a scanned candidate is unpinned but has not completed write-through (no `ssd_offset`), `evict_for_space` drops it entirely via `dm.remove` + `mt.remove` (data loss — the block is recomputed on next miss) rather than demoting it. If a candidate has active read/write references (pinned), it is skipped and never freed; if every candidate scanned within the widening `oldest_keys` window is pinned, `evict_for_space` returns `AllocationFailed` instead of blind-freeing a pinned slot (see FR-024).
- When the SSD evictor runs but all candidate entries have active references, no entries are evicted in that sweep. The evictor re-checks on its next interval.
- When the SSD evictor removes an entry, it frees the extent via the extent manager and removes the dispatch-map entry. No memory-tier operation is needed (the entry was already evicted from DRAM).
- When a data drive's partition table has fewer than the expected 3 Certus partitions (metadata, extended metadata, data) — e.g. a valid-but-non-Certus GPT such as an empty or foreign disk — `initialize()` returns `DispatcherError::IoError` with the drive index and remediation guidance instead of panicking on an out-of-bounds partition index (see FR-055).

## Clarifications

### Session 2026-05-22 (Eviction Refinement & Background Writer)

- Q: How does `evict_for_space` choose which entry to evict? -> A: *(superseded 2026-07-20 — see Session 2026-07-22 below and FR-024)* Sparse-probe plus shard-targeted-LRU-primary. The function signature is `evict_for_space(dm, mt, needed, target_key)`. The primary path on most iterations calls `mt.evict_lru_for_key(target_key)` which evicts the LRU entry from the same shard as the target key (O(1) per-shard lock). This ensures the freed space is in the correct shard for the subsequent `insert(target_key, ...)` — the memory-tier uses 16 shards and `insert` allocates from `shard = key % 16`, so untargeted eviction from other shards would waste effort. On every 8th iteration a small clean-eviction probe queries `mt.oldest_keys(4)` (MAX_SCAN=4) for LRU candidates and checks each via `dm.is_evictable(key)`. `is_evictable` returns true when the entry is in MemoryTier state with `ssd_offset: Some(_)` (write-through complete) and no active read/write references. If a clean candidate is found it is preferred: `mt.remove(key)` + `convert_memory_tier_to_block`. The loop runs until `used + needed <= capacity` or MAX_ATTEMPTS=512 iterations are exhausted (returns AllocationFailed).
- Q: Why does the background writer use `peek()` instead of `get()`? -> A: `IMemoryTier::peek()` returns the pointer and size without refreshing the entry's LRU position. This ensures background write-through does not artificially keep entries "hot" and prevent their eviction under memory pressure.
- Q: What is `IMemoryTier::oldest_keys(n)`? -> A: Returns up to N keys in LRU order (oldest first) without removing them. Used by `evict_for_space` to scan for eviction candidates.
- Q: What is `IDispatchMap::is_evictable(key)`? -> A: Returns true if the entry exists in MemoryTier state with a non-None `ssd_offset` and has no active read or write references. Used by `evict_for_space` to identify safe eviction candidates.

### Session 2026-05-12 (SSD Eviction)

- Q: What about `max_cache_entries` and `eviction_threshold` in DispatcherConfig? -> A: These are vestigial from the v0 count-based eviction and are unused in v1. Memory-tier eviction is purely capacity-based (FR-024). They are retained in the struct for API backward compatibility but should be considered deprecated.
- Q: How does the SSD evictor determine drive ownership for extent removal? -> A: Uses `key % num_drives` to identify the target drive, matching the write-through path's drive selection.

### Session 2026-05-08 (Memory-Tier Rewrite)

- Q: How is the memory-tier pool managed? -> A: *(partially superseded 2026-07-20 — eviction mechanism now pin-safe, see Session 2026-07-22 below and FR-024; shard/pool-management details below remain accurate)* The `IMemoryTier` interface provides `insert()`, `get()`, `peek()`, `remove()`, `evict_lru()`, `evict_lru_for_key()`, `oldest_keys()`, `touch()`, `capacity()`, and `used()`. The `peek()` method returns the pointer and size without refreshing LRU (used by the background writer). The `oldest_keys(n)` method returns up to N keys in LRU order without removing them (used by `evict_for_space`). The `evict_lru_for_key(key)` method evicts the LRU entry from the same shard as `key`, ensuring the freed space is allocatable by a subsequent `insert(key, ...)`. The pool is pre-allocated DRAM with 16 shards (key % 16). Eviction is capacity-based: `used + needed > capacity` triggers eviction (as of 2026-07-20, `evict_for_space` no longer calls the shard-targeted `evict_lru_for_key` blind fallback — see FR-024).
- Q: What happens when an evicted entry is looked up? -> A: The dispatch-map returns `LookupResult::BlockDevice { offset }`. The dispatcher calls `promote_and_serve` which reads from SSD, inserts into memory-tier, DMA-copies to GPU, and re-registers in the dispatch-map.
- Q: What happens if write-through hasn't completed when eviction occurs? -> A: *(superseded 2026-07-20 — see Session 2026-07-22 below and FR-024)* The sparse-probe eviction (every 8th attempt) preferentially selects entries where `is_evictable()` is true (write-through complete, no active references). Entries with incomplete write-through are evicted on all other iterations via shard-targeted `evict_lru_for_key(target_key)`. In that case, `convert_memory_tier_to_block` fails (no ssd_offset set), the dispatch-map entry is removed entirely, and the data is effectively lost. This is accepted as an infrequent occurrence in practice; under normal load most entries have completed write-through before eviction pressure arises.

### Session 2026-07-22 (Pin-Safe Eviction, Per-Drive Channel Pool, Partition-Table Guard)

- Q: How does `evict_for_space` choose which entry to evict, now that the blind `evict_lru`/`evict_lru_for_key` fallback is gone? -> A: `evict_for_space(dm, mt, needed, _target_key, max_attempts)` frees one victim per iteration via `evict_one_clean`, which scans `IMemoryTier::oldest_keys(scan)` over a window that widens with sustained pressure (`MAX_SCAN=4 × attempts`, capped at 1024) to look past pinned entries deeper in the LRU. Each candidate is either demoted to BlockDevice (write-through complete, unpinned) or dropped entirely (unpinned, write-through incomplete — data loss). Entries with active read/write references are never freed by either branch, so eviction can no longer reclaim a slot a pinned in-flight load points at. `target_key` (retained for API compatibility) is unused; shard-targeting was removed along with the blind fallback. If every scanned candidate is pinned, the function returns `AllocationFailed` instead of blind-freeing. See FR-024.
- Q: Why was the shared per-drive `ClientChannels` cache replaced with a `ChannelPool`? -> A: A single cached channel per drive let two concurrent readers on the same drive (`batch_lookup` and the `promote_to_memory_tier` prefetch path) dequeue each other's NVMe completions ("completion theft"), causing a permanent hang. `ChannelPool` lazily grows a per-drive pool of exclusive, RAII-leased channels so each concurrent reader gets its own; checkout drains stale completions to prevent an old op's tags from being matched against a new op's segments. See FR-034.
- Q: What happens if a data drive has a partition table that isn't a Certus layout (e.g. an empty or foreign GPT)? -> A: `initialize()` validates the recovered partition table has the expected 3 Certus partitions (metadata, extended metadata, data) before indexing into it, returning a descriptive `DispatcherError::IoError` instead of panicking with an out-of-bounds index. See FR-055.
- Q: Does lookup update the LRU order? -> A: Yes, `mt.touch(key)` is called after a successful MemoryTier lookup to refresh the entry's position in the LRU.
- Q: How does the pipelined reader work? -> A: The primary path (`pipelined_ssd_to_gpu_zero_copy`) uses a sliding-window pipeline: maintains a FIFO queue of up to `max_queue_depth` in-flight NVMe reads into unique offsets of the memory-tier slot (CUDA-pinned + SPDK-registered). As each NVMe read completes, a GPU H2D async copy (`dma_copy_to_device_async`) is issued immediately for that segment on alternating CUDA streams, and the next NVMe read is submitted right after — overlapping SSD I/O with GPU DMA. Correctness assumes FIFO completion ordering within a single NVMe queue pair (completions are matched to the oldest in-flight segment via `VecDeque::pop_front`). A periodic stream sync every 16 GPU commands bounds the CUDA command queue depth. Final sync on both streams before returning. The fallback path (`pipelined_ssd_to_gpu`) uses intermediate ring buffers (8 CUDA-pinned DmaBuffers) and processes in batches: submit all reads for a batch → wait for all completions → memcpy each to memory-tier slot + issue GPU copies → next batch.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST define an `IDispatcher` interface in the shared interfaces crate. *(Sync 2026-08-07 — backfilled to the full shipped method inventory.)* The interface provides:
  - **Lifecycle**: `initialize`, `shutdown`.
  - **Lookup / read path**: `lookup`, `lookup_async`, `batch_lookup`, `check`.
  - **Write / cache management**: `populate`, `remove`, `touch`, `promote_to_memory_tier`.
  - **GPU-staged memory-lifecycle primitives** (see FR-056): `reserve_memory`, `copy_gpu_to_memory_async`, `copy_gpu_to_memory_completed`, `release_memory`, `pin`, `unpin`.
  - **Durability / introspection** (see FR-056): `flush_to_ssd`, `read_write_stats`.

  The eviction-notification methods `create_eviction_channel` and `eviction_dropped_count` (FR-042) are inherent methods on the concrete `DispatcherComponent` rather than trait methods on `IDispatcher`.
- **FR-002**: System MUST define a `DispatcherError` error type in the shared interfaces crate, covering all failure modes (not initialized, key not found, duplicate key, I/O error, allocation failure, invalid parameter).
- **FR-003**: The `populate(key, ipc_handle)` method MUST allocate a slot in the DRAM memory-tier via `IMemoryTier::insert()`, initiate DMA copy from the client's GPU memory into the memory-tier slot via `IGpuServices::dma_copy_to_host`, register the entry in the dispatch map as a MemoryTier entry (via `create_memory_tier_entry`), and enqueue an asynchronous background write-through job. MUST run `evict_for_space` if the memory-tier pool lacks capacity.
- **FR-004**: After a successful populate, the system MUST asynchronously read from the memory-tier pointer (via `IMemoryTier::peek()` to avoid refreshing LRU order) and write the data to the SSD via the block device and extent manager (write-through), calling `convert_to_storage` on completion to record the SSD offset.
- **FR-005**: The memory-tier entry MUST remain accessible for fast lookups even after the background SSD write-through completes. The memory-tier slot is NOT freed on write-through completion.
- **FR-006**: The `lookup(key, ipc_handle)` method MUST query the dispatch map: if MemoryTier, DMA from memory-tier pointer to GPU and touch LRU; if BlockDevice (evicted), promote back to memory-tier via pipelined SSD-to-DRAM-to-GPU reader and re-register as MemoryTier. When the memory-tier pool is CUDA-pinned (registered via FR-035), the MemoryTier path SHOULD use `IGpuServices::memcpy_h2d_async` on the dedicated warm stream (FR-037) with raw memory-tier pointers for the H2D transfer, avoiding DmaBuffer wrapping overhead. Falls back to synchronous `dma_copy_to_device` when no CUDA streams are available.
- **FR-007**: The `lookup` method MUST return a cache-miss indication if the key does not exist in the dispatch map.
- **FR-008**: The `check(key)` method MUST return whether a cache entry exists for the given key without performing any data transfer.
- **FR-009**: The `remove(key)` method MUST free the memory-tier slot (if present via `mt.remove()`), free the extent on SSD (if in BlockDevice state), and remove the dispatch map entry.
- **FR-010**: The dispatcher component MUST use the component framework's `define_component!` macro and expose only the `IDispatcher` interface.
- **FR-011**: The dispatcher MUST accept receptacles for `ILogger`, `IDispatchMap`, `IGpuServices`, `ISPDKEnv`, `IMemoryTier`, and `IRemoteLookup` components. Block devices and extent managers are created internally during initialization. `DispatcherConfig` MUST include an optional `poller_base_cpu: Option<usize>` field. When set, each data drive's NVMe poller actor is pinned to CPU core `poller_base_cpu + drive_index` via `IBlockDeviceAdmin::set_actor_cpu`. This is used for NUMA-aware deployments. Defaults to `None` (OS scheduler decides). The `IRemoteLookup` receptacle is used for forwarding cache misses to remote Certus nodes; when unbound, remote lookup is skipped. The dispatcher calls `IRemoteLookup::batch_lookup` with `(key, size)` pairs (not `IpcHandle` — remote-lookup works only in DRAM); on a successful remote fetch the value becomes resident in the local memory tier, and the dispatcher then performs the DRAM→GPU delivery for that key using the same memory-tier→device copy as a local hit.
- **FR-012**: The `initialize` method MUST validate that the `dispatch_map` and `memory_tier` receptacles are bound before proceeding. Other receptacles are validated lazily on first use.
- **FR-013**: The dispatcher MUST use appropriate read/write locking on the dispatch map to ensure thread safety during concurrent operations.
- **FR-014**: The `shutdown` method MUST ensure all in-flight background operations complete or are cancelled before returning. Before shutting down block devices, the dispatcher MUST checkpoint all extent managers (via `IExtentManager::checkpoint()`) to persist their metadata to SSD.
- **FR-015**: The dispatcher MUST coordinate N data block devices with N extent managers. Each extent manager stores its metadata (superblock + bitmap + records) on the same data block device it manages — there is no separate metadata device.
- **FR-016**: The dispatcher MUST pass the data block device size and computed FormatParams to each extent manager's format function.
- **FR-017**: When the asynchronous background write-through fails (extent allocation failure or block device I/O error), the background writer silently drops the job. The dispatch map entry remains in MemoryTier state. (Known limitation.)
- **FR-018**: The `remove(key)` method does NOT block waiting for background write-through to complete. It proceeds immediately with removal.
- **FR-019**: All block device I/O operations MUST be segmented to respect MDTS (typically 128 KiB). The promotion path uses zero-copy pipelines (`pipelined_ssd_to_gpu_zero_copy` for single-entry, `pipelined_multi_object_zero_copy` for batch) with a configurable `max_queue_depth` parameter. The pipeline maintains up to `max_queue_depth` concurrent in-flight NVMe reads directly into unique offsets of the memory-tier slot (CUDA-pinned + SPDK-registered via FR-035). As each NVMe read completes, the GPU H2D async DMA copy for that segment is issued immediately and the next NVMe read is submitted — overlapping SSD I/O with GPU DMA. Tag-based completion routing (`obj_idx * max_segments + seg_idx`) identifies which segment completed for out-of-order handling. A periodic stream sync every `PIPELINE_RING_SIZE` (8) GPU commands bounds the CUDA command queue depth. The single-entry `promote_and_serve` path uses `max_queue_depth=16`. The `batch_lookup` path uses `max_queue_depth=128` per thread to saturate the NVMe submission queue for maximum per-drive parallelism. Memory-tier SPDK+CUDA registration (FR-034) is required for the zero-copy paths. These pipelines drain every submitted read to completion and do not break early on error (see FR-054).
- **FR-020**: ~~REMOVED~~ *(superseded 2026-07-02)* — The prepare_store/commit_store/cancel_store direct-write workflow has been removed. Data ingestion uses the GPU DMA memory-tier path exclusively.
- **FR-021**: ~~REMOVED~~ *(superseded 2026-07-02)* — See FR-020.
- **FR-022**: ~~REMOVED~~ *(superseded 2026-07-02)* — See FR-020.
- **FR-023**: The `touch(key)` method MUST update the entry's eviction timestamp in the dispatch map AND refresh the memory-tier LRU position (if the entry is memory-tier resident) without performing any DMA transfer or acquiring any dispatch-map reference. MUST return `KeyNotFound` if the key does not exist.
- **FR-024**: Eviction in v1 is purely capacity-based within the memory-tier pool and MUST be pin-safe. When the pool is full, `evict_for_space(dm, mt, needed, _target_key, max_attempts)` frees one victim per iteration via `evict_one_clean`, which scans `IMemoryTier::oldest_keys(scan)` where the scan widens as pressure persists (`MAX_SCAN=4 × attempts`, capped at 1024) to look past pinned/unpersisted entries deeper in the LRU. For each candidate, in order of preference: (1) **demote** to BlockDevice via `IDispatchMap::try_evict_to_block` (write-through complete, unpinned) followed by `IMemoryTier::remove`; (2) otherwise, if unpinned, **drop** it entirely via `dm.remove` + `mt.remove` (write-through incomplete — the block is lost from cache and recomputed on next miss). In BOTH branches the dispatch-map transition happens BEFORE the DRAM slot is freed, and both dispatch-map operations reject entries with active read/write references, so eviction NEVER frees a slot a pinned in-flight load still points at. The prior blind `evict_next`/`evict_next_for_key(target_key)` fallback (which could reclaim a pinned slot and corrupt a concurrent load) has been REMOVED; `target_key` is unused. If every scanned candidate is pinned, `evict_for_space` returns `AllocationFailed` rather than blind-free; the caller leaves the block uncached (`populate`) or serves it via the staging pool (FR-053). The loop is bounded by `max_attempts` (configurable via `DispatcherConfig::max_eviction_attempts`, default 2048). `evict_and_insert`'s shard-fragmentation relief path likewise force-evicts one pin-safe victim via `evict_one_clean` and fails rather than blind-free when none is evictable. Count-based TSC eviction (from v0) is NOT used in v1.
- **FR-025**: The `DispatcherConfig` MUST support a `format_on_init` flag (default true). When false, extent managers are not reformatted on initialization, preserving on-disk data from previous sessions. Additionally, when `format_on_init=false`, after recovering all extent managers, the dispatcher MUST reconstruct the dispatch map by iterating each extent manager's allocated extents via `IExtentManager::for_each_extent` and calling `IDispatchMap::recover_extent(key, offset, size)` for each. This restores the full cache index from persisted SSD metadata. The number of recovered extents and elapsed time SHOULD be logged.
- **FR-026**: ~~REMOVED~~ *(superseded 2026-05-21)* — Block device version selection is no longer required. The implementation hardcodes a single block device component; there is no version multiplexing.
- **FR-027**: ~~REMOVED~~ *(superseded 2026-05-21)* — Extent manager version selection is no longer required. The implementation hardcodes a single extent manager; there is no version multiplexing.
- **FR-028**: On BlockDevice lookup (promotion), the pipelined reader MUST re-insert the entry into the memory-tier and re-register it as a MemoryTier entry in the dispatch map.
- **FR-029**: The dispatcher MUST start a background SSD evictor thread during `initialize()` if `ssd_eviction_threshold > 0.0` and at least one data drive is configured. The evictor MUST be shut down (thread joined) during `shutdown()`.
- **FR-030**: The SSD evictor MUST periodically check combined SSD utilization (sum of `IExtentManager::used_bytes()` / sum of `IExtentManager::capacity_bytes()` across all extent managers). The check interval MUST be configurable via `ssd_eviction_interval_secs` (default: 5 seconds).
- **FR-031**: When SSD utilization exceeds `ssd_eviction_threshold` (default: 0.9), the evictor MUST evict BlockDevice-only entries using `IDispatchMap::oldest_keys(batch_size)` for LRU ordering, stopping when utilization drops below `ssd_eviction_low_watermark` (default: 0.8) or the batch is exhausted.
- **FR-032**: The SSD evictor MUST skip entries in MemoryTier state (still hot in DRAM). Entries with active read or write references MUST be skipped gracefully (dm.remove fails without panic).
- **FR-033**: The `DispatcherConfig` MUST include `ssd_eviction_threshold` (f64, default 0.9), `ssd_eviction_low_watermark` (f64, default 0.8), `ssd_eviction_batch_size` (usize, default 64), `ssd_eviction_interval_secs` (u64, default 5), `max_eviction_attempts` (usize, default 2048), `memory_tier_eviction_threshold` (f64, default 0.0), `memory_tier_eviction_low_watermark` (f64, default 0.70), `memory_tier_eviction_batch_size` (usize, default 64), `memory_tier_eviction_interval_secs` (u64, default 2), `cold_staging_slots` (usize, default 64), and `cold_staging_buf_bytes` (usize, default 4 MiB = 4 × 1024 × 1024). Setting `ssd_eviction_threshold` to 0.0 disables the SSD evictor. Setting `memory_tier_eviction_threshold` to 0.0 disables the memory-tier evictor. Setting `cold_staging_slots` to 0 disables the cold-load staging pool (FR-053). `max_eviction_attempts` controls how many iterations `evict_for_space` attempts before returning `AllocationFailed`. *(Sync 2026-08-07 — backfilled additional shipped `DispatcherConfig` fields: `metadata_partition_size` (u64, default 128 MiB) and `extended_metadata_partition_size` (u64, default 128 MiB), which size the metadata and extended-metadata GPT partitions created during `initialize()`; and `backfill_delay_ms` (u64, default 10), a peer-to-peer/remote-replication tuning knob consumed only on the dispatcher's backfill path and inert for purely local caching. These are part of the shared `DispatcherConfig` struct in the interfaces crate.)*
- **FR-034**: During `initialize()`, after GPU and memory-tier are ready, the dispatcher MUST call `IGpuServices::register_host_memory` on the memory-tier pool to CUDA-pin and SPDK-register it for zero-copy NVMe and GPU DMA. Registration failure MUST be logged as a warning (zero-copy pipelines require this registration). Each data drive's `ClientChannels` MUST be managed via a per-drive `ChannelPool` rather than a single cached instance: the pool starts empty at `initialize()` and grows on demand — each concurrent reader (`promote_and_serve`, `serve_cold_staged`, and each `promote_to_memory_tier` per-drive prefetch thread) checks out an exclusive channel via `ChannelPool::checkout()`, which connects a fresh `ClientChannels` on first use or reuses one returned by a prior lease, and returns a `ChannelLease` (RAII) that puts the channel back in the pool on drop. Checkout MUST drain any completions left in `completion_rx` by a previous user before handing out the channel, so a recycled channel's stale tags are never matched against a new caller's segments. `connect_client()` MUST be called outside the pool's lock so a slow connect does not block other checkouts. This replaces a prior design that cached a single `ClientChannels` per drive at init time: a single shared SPSC channel let two concurrent readers on the same drive (e.g. `batch_lookup` and the `promote_to_memory_tier` prefetch path) dequeue each other's NVMe completions ("completion theft"), causing a permanent, non-timing-out hang. The pool's lazy, per-checkout connection is a deliberate trade-off of a small first-use latency for correctness under concurrent per-drive access.
- **FR-035**: During `shutdown()`, before memory-tier teardown, the dispatcher MUST call `IGpuServices::unregister_host_memory` to release CUDA page-locking and SPDK registration on the memory-tier pool.
- **FR-036**: System MUST provide a `lookup_async(key, ipc_handle) -> Result<GpuStream, DispatcherError>` method on the `IDispatcher` interface. This method performs the same cache lookup as `lookup` (dispatch-map query, memory-tier hit, BlockDevice promotion) but returns a `GpuStream` handle instead of blocking on the H2D DMA transfer. The caller MUST call `stream_synchronize` on the returned stream before accessing the GPU destination memory. For SSD promotion paths the copy completes synchronously and a null stream is returned. The synchronous `lookup` method MUST delegate to `lookup_async` internally and call `stream_synchronize` before returning.
- **FR-037**: The dispatcher MUST pre-allocate a single warm CUDA stream during `initialize()` for the memory-tier lookup hot path. The stream handle is stored as an `AtomicU64` and used by `lookup_async` and `batch_lookup` for `memcpy_h2d_async` on raw memory-tier pointers. The single `AtomicU64` warm stream (`warm_stream`) is retained as a fallback, but the warm memory-tier hot path (`lookup_async`, `batch_lookup`) and the D2H populate path now resolve the destination GPU device from the client's IPC pointer and use the per-device warm stream from the process-global `DEVICE_STREAMS` map (see FR-052). A CUDA stream is bound to the device that was current at creation, so a stream on another GPU makes `cudaMemcpyAsync` fail with "invalid argument" under multi-GPU / tensor parallelism; per-device streams avoid this. The warm stream MUST be destroyed during `shutdown()`.
- **FR-038**: System MUST provide a `clear_memory_tier() -> Result<usize, DispatcherError>` method on the `IDispatcher` interface. This method evicts ALL entries from the memory-tier pool by calling `IMemoryTier::evict_next()` in a loop until empty. For each evicted key, it transitions the dispatch-map entry to BlockDevice state via `convert_memory_tier_to_block`. If the transition fails (write-through not complete), the entry is removed from the dispatch map entirely. Returns the number of entries cleared. The `IMemoryTier` trait MUST also provide a `clear() -> Result<usize, MemoryTierError>` method that resets the pool (clears slots, eviction-order state, and allocator) in a single operation.
- **FR-039**: System MUST provide a `batch_lookup(entries: &[(CacheKey, Vec<IpcHandle>)]) -> Vec<Result<(), DispatcherError>>` method on the `IDispatcher` interface. *(Sync 2026-08-07 — signature backfilled: each entry now carries a `Vec<IpcHandle>` of destination regions, not a single `IpcHandle`, so one cache key can be scattered to multiple GPU destinations (e.g. sibling tensor-parallel ranks) in a single batched operation.)* This method processes a batch of lookup entries concurrently: (1) classifies all entries by dispatch-map state, (2) for MemoryTier hits, issues all `memcpy_h2d_async` calls on the warm stream (FR-037) and performs a single deferred `stream_synchronize` after all hot-path copies are issued (batched sync for throughput), (3) groups BlockDevice (cold) entries by target drive using `key % num_drives`, (4) spawns up to `MAX_QUEUES_PER_DRIVE` (default 2) threads per drive using `std::thread::scope`, each with its own NVMe client channels and CUDA streams, (5) each thread calls `pipelined_multi_object_zero_copy` with `max_queue_depth = 128` per thread, (6) merges results back in input order. Returns one `Result` per input entry.
- **FR-040**: System MUST provide `promote_to_memory_tier(keys: &[CacheKey])` on the `IDispatcher` interface. This is a best-effort, fire-and-forget method that asynchronously promotes SSD-resident (BlockDevice) entries to the memory-tier without GPU DMA. For each key: if in BlockDevice state, reads data from SSD into a new memory-tier slot (via `pipelined_ssd_to_dram_only`) and updates the dispatch-map to MemoryTier; if in MemoryTier or Staging, refreshes the eviction timestamp; if not found, silently skips. Errors on individual keys are logged via ILogger but not propagated. The gRPC handler spawns this as a detached background task when `BatchTouchRequest.promote = true`.
- **FR-041**: The pipeline module MUST provide `pipelined_ssd_to_dram_only` and `pipelined_multi_ssd_to_dram_only` functions that perform pipelined NVMe reads directly into memory-tier slots without any GPU DMA. These use the same NVMe queue depth saturation strategy as FR-019 (submit up to `max_queue_depth` reads, sliding-window completion loop) but omit all CUDA stream management and GPU copy operations. The `promote_to_memory_tier` implementation currently calls `pipelined_ssd_to_dram_only` for each entry sequentially per drive thread; the multi-object variant exists for future batch promotion optimization.
- **FR-042**: The dispatcher MUST provide an eviction event notification channel via `create_eviction_channel(capacity: usize) -> crossbeam_channel::Receiver<EvictionEvent>`. *(Sync 2026-08-07 — signature backfilled: the caller now supplies the bounded channel `capacity`; the channel is created via `crossbeam_channel::bounded(capacity)` and the sender is installed internally.)* Eviction events are emitted (best-effort, non-blocking send) whenever an entry is evicted from the memory-tier by `evict_for_space`. The channel is bounded; when the receiver falls behind, events are dropped and a dropped-event counter is incremented (queryable via `eviction_dropped_count()`). This mechanism enables external consumers (e.g., gRPC TakeEvents stream) to observe cache evictions without polling.
- **FR-043**: The dispatcher MUST define a `PipelineMetrics` trait providing timing observation hooks for internal pipeline stages (cold SSD read latency, hot GPU DMA latency, populate phase duration). An implementation of `PipelineMetrics` MAY be injected to record per-operation telemetry. When no metrics implementation is provided, timing observations are no-ops.
- **FR-044**: The dispatcher MUST provide a `ColdReadPool` — a persistent worker pool with pre-connected NVMe client channels and CUDA streams — for cold-path reads in `batch_lookup`. The pool pre-allocates per-drive, per-queue resources at initialization time to avoid per-batch connection overhead. `batch_lookup` dispatches cold entries to the pool instead of spawning scoped threads for each batch (FR-039 describes the logical parallelism; ColdReadPool is the resource-management optimization).
- **FR-045**: When `batch_lookup` encounters entries not found in the local dispatch map and the `IRemoteLookup` receptacle is bound (FR-011), the dispatcher MUST forward those cache-miss keys to `IRemoteLookup` for resolution against remote Certus nodes. Results from remote lookup are merged back into the batch response in input order. When the receptacle is unbound, cache misses remain as `KeyNotFound` errors.
- **FR-046**: The dispatcher MUST start a background memory-tier evictor thread during `initialize()` if `memory_tier_eviction_threshold > 0.0`. The evictor MUST be shut down (thread joined) during `shutdown()`.
- **FR-047**: The memory-tier evictor MUST use `memory_tier_eviction_interval_secs` (default: 2 seconds) as the idle polling interval when utilization is below threshold. When above threshold, the evictor MUST loop continuously with a 200ms pace between successful demotion batches and a 500ms backoff when no entries are evictable (to allow write-through to release references).
- **FR-048**: When memory-tier utilization exceeds `memory_tier_eviction_threshold` (default: disabled at 0.0), the evictor MUST scale the effective batch size using a quadratic pressure curve: `multiplier = 1.0 + 7.0 × pressure²` where `pressure = (utilization - threshold) / (1.0 - threshold)`, giving 1× to 8× the configured `batch_size`. On consecutive dry runs (no entries evictable), the scan window MUST widen up to 4× the effective batch to find evictable entries deeper in the LRU list. Demotion stops when utilization drops below `memory_tier_eviction_low_watermark` (default: 0.70) or the scan is exhausted.
- **FR-049**: For each candidate, the evictor MUST call `IDispatchMap::try_evict_to_block(key)` which atomically verifies evictability (write-through complete, no active references) and transitions the entry to BlockDevice state under a single lock hold. Only after the dispatch-map reflects BlockDevice state MUST the DRAM slot be freed via `IMemoryTier::remove(key)`. This ordering prevents a race where concurrent lookups obtain a freed memory-tier pointer. If `try_evict_to_block` fails (active references or write-through not complete), the entry MUST be skipped — the evictor does NOT remove the dispatch-map entry on transition failure. The entry remains in MemoryTier state and will be re-evaluated on subsequent sweeps.
- **FR-050**: The evictor MUST emit `EvictionEvent { key, reason: Demoted }` (or `Removed` on transition failure) via the eviction notification channel (FR-042) for each demoted entry.
- **FR-051**: When a `batch_lookup` cold promotion loses the `IMemoryTier::insert` race to a concurrent lookup for the same key (receiving `MemoryTierError::AlreadyExists`), the dispatcher MUST treat this as a hit rather than a failure. The error is mapped to `DispatcherError::AlreadyExists` and, in a post-classification recovery pass (`serve_concurrently_promoted`), the dispatcher bounded-waits (up to 5s, 50µs backoff) for the winning promoter to transition the dispatch-map entry to `MemoryTier`, then serves the resident slot to the GPU via `serve_memory_tier_to_gpu` (per-device warm stream, FR-052). If the entry is still `BlockDevice` at timeout it returns `KeyNotFound`; `MismatchSize` returns `InvalidParameter`. The caller's load pin keeps the entry from being evicted while waiting. This prevents spurious `AlreadyExists` load failures when sibling tensor-parallel ranks request the same content-hash key concurrently.
- **FR-052**: Cold (SSD→GPU) and warm (memory-tier→GPU) DMA transfers MUST use CUDA streams bound to the destination GPU's device. Because `cudaMemcpyAsync` rejects a peer pointer on a different device than its stream ("invalid argument") under multi-GPU / tensor parallelism, the dispatcher maintains per-device streams in a process-global `DEVICE_STREAMS` map — one warm stream plus one pipeline stream pair per device, created lazily on the target device via `device_streams_for`. It resolves each request's device from its IPC destination pointer (`IGpuServices::device_of_ptr` / `set_batch_device`), makes that device current on the issuing thread, and selects the streams bound to it. `batch_lookup` resolves one device for the whole batch (all entries in an RPC come from a single rank); `cold_pool::ColdReadRequest` carries a `gpu_device: i32` field so the pool worker calls `IGpuServices::set_device` and uses device-bound streams. When the device is unknown (`-1`) the paths fall back to the shared pipeline-ring / `warm_stream` streams.
- **FR-053**: The dispatcher MUST provide a bounded cold-load staging pool (`StagingPool`) of `cold_staging_slots` pre-registered, CUDA-pinned + SPDK-registered host DRAM buffers of `cold_staging_buf_bytes` each, leased via an RAII `StagingLease` that returns the buffer on drop. When a cold (BlockDevice) load cannot obtain a memory-tier slot because the tier is saturated (`evict_for_space` / `evict_and_insert` returns `AllocationFailed` and at least one data drive is configured), the dispatcher MUST serve the read through a staging buffer (`SSD → staging → GPU` via `serve_cold_staged`) and leave the dispatch-map entry as `BlockDevice` (served uncached, NOT promoted) rather than failing the load. In `batch_lookup` such entries are deferred to a post-pass that runs after the pooled tier jobs and serves them one lease at a time while holding no other lock, so the blocking `checkout` is deadlock-free. This bounds staging memory so a burst of concurrent cold reads cannot exhaust the memory tier and cause fatal `AllocationFailed`. Setting `cold_staging_slots` to 0 disables staging. (Config fields: see FR-033.)
- **FR-054**: The pipelined cold-read paths (`pipelined_ssd_to_gpu_zero_copy`, `pipelined_multi_object_zero_copy`) MUST drain every submitted NVMe read to completion (loop until `completed == submitted`) before returning and MUST NOT break early on the first error. On error the path sets a `stop_submitting` flag — submitting no new reads and marking any un-submitted work as failed for its object — but continues draining outstanding completions; the first error is retained and returned. Breaking early while reads are still in flight would orphan completions in the client's SPSC completion ring and deadlock (hang) the next client that reuses the NVMe queue.
- **FR-055**: During `initialize()`, after `initialize_or_format` returns a partition table for a data drive (FR-025), the dispatcher MUST validate that the table contains at least `EXPECTED_PARTITIONS` (3: metadata, extended metadata, data) entries before indexing `table.partitions[0]` (metadata base LBA) and `table.partitions[2]` (data base LBA). A disk with a valid-but-non-Certus GPT (e.g. empty, zeroed, or foreign) reads back with fewer than 3 partitions; without this guard, indexing panics with an out-of-bounds access instead of failing gracefully. If the check fails, `initialize()` MUST return `DispatcherError::IoError` naming the drive index, the expected and actual partition counts, and remediation guidance (re-run with `format_on_init` to destructively reformat the drive) rather than panicking.

- **FR-056** *(Sync 2026-08-07 — backfilled; these methods ship on `IDispatcher` but had no requirement)*: The dispatcher MUST additionally expose the following `IDispatcher` methods:
  - **GPU-staged memory-lifecycle primitives** — an explicit reserve→copy→complete flow that lets a caller populate a cache entry from GPU memory across one or more regions without the single-shot `populate` call:
    - `reserve_memory(key, size, session_id) -> *mut u8` reserves a memory-tier slot of `size` bytes for `key`, tags it with `session_id`, and returns the host pointer for staging.
    - `copy_gpu_to_memory_async(key, regions: &[IpcHandle], stream)` issues asynchronous GPU→host DMA of the given IPC regions into the reserved slot on the supplied CUDA stream.
    - `copy_gpu_to_memory_completed(key, size)` finalizes the entry after the async copies complete, transitioning it to a resident `MemoryTier` entry (design invariant P10).
    - `release_memory(key)` releases a reserved-but-not-completed slot.
    - `pin(key)` / `unpin(key)` add/remove a load pin that prevents the entry from being evicted while a caller holds it.
  - **Durability / introspection**:
    - `flush_to_ssd() -> usize` forces a synchronous write-through flush of pending dirty entries to SSD and returns the number of entries flushed (design invariant P9).
    - `read_write_stats() -> ReadWriteStats` returns aggregate read/write I/O counters (the `ReadWriteStats` type is shared from `iblock_device`).

- **FR-057** *(Sync 2026-08-20 — backfilled; the dependency-injection / test surface shipped with no requirement)*: To enable component testing without SPDK/NVMe hardware, the concrete `DispatcherComponent` MUST expose inherent dependency-injection setters that override its internally-constructed dependencies. These are inherent methods on `DispatcherComponent` (NOT `IDispatcher` trait methods), each guarded so that when it is not called the dispatcher falls back to its default hard-coded implementation:
  - `set_block_device_factory(factory: BlockDeviceFactory)` — installs a factory the dispatcher uses, in place of the hard-coded SPDK NVMe block-device implementation, when creating the N data block devices during `initialize()`.
  - `set_extent_manager_factory(factory: ExtentManagerFactory)` — installs a factory the dispatcher uses, in place of the default extent-manager implementation, when creating the N extent managers during `initialize()`.
  - `set_pipeline_metrics(m: Arc<dyn PipelineMetrics>)` — installs a `PipelineMetrics` implementation (FR-043) so that internal data-path stages report timing through the trait; when no implementation is installed, timing observations are no-ops.

  These setters let unit/integration tests inject mock block-device and extent-manager components and observe pipeline telemetry against a dispatcher that is otherwise driven through its normal lifecycle, with no NVMe hardware present.

### Key Entities

- **CacheKey**: A 64-bit identifier for cached data elements. Used to address entries in the dispatch map and memory-tier.
- **IPC Handle**: Contains a pointer to a GPU memory region and a size. Used for DMA transfers between GPU and host memory.
- **Memory-Tier Slot**: A region in the pre-allocated DRAM pool managed by `IMemoryTier`. Holds cached data for fast access. Eviction is capacity-based (bytes used vs pool size).
- **Dispatch Map Entry**: A record tracking the state of a cached element -- MemoryTier (pointer + size + optional ssd_offset) or BlockDevice (ssd_offset only).
- **Extent**: A contiguous region on a data block device, managed by the extent manager, used to store write-through cache data.
- **Data Block Device**: An NVMe SSD that holds persisted cache data. There are N data block devices in the system.
- **Background Writer**: A dedicated thread that processes write-through jobs, reading from memory-tier pointers via `IMemoryTier::peek()` (to avoid refreshing LRU) and writing to SSD via extent managers.
- **Pipelined Reader**: A sliding-window zero-copy pipeline (`pipeline.rs`) that maintains up to `max_queue_depth` concurrent in-flight NVMe reads into unique memory-tier offsets. As each read completes, an async GPU H2D copy is issued for that segment and the next NVMe read is submitted immediately, overlapping SSD I/O with GPU DMA. Two variants: `pipelined_ssd_to_gpu_zero_copy` (single object) and `pipelined_multi_object_zero_copy` (batch of N objects with tag-based completion routing). Memory-tier SPDK+CUDA co-registration is required.
- **Background SSD Evictor**: A dedicated thread that periodically checks SSD utilization and evicts the oldest BlockDevice entries when capacity exceeds a threshold.
- **Background Memory-Tier Evictor**: A dedicated thread that proactively demotes LRU entries from DRAM memory-tier to SSD-backed BlockDevice state when DRAM utilization exceeds a configurable threshold, preventing on-demand eviction stalls on the populate hot path.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A client can populate a cache entry and subsequently retrieve it via lookup (memory-tier hit path), receiving the correct data, within a single session.
- **SC-002**: Cache check operations return accurate presence information for both existing and non-existing keys.
- **SC-003**: Cache removal frees all associated resources (memory-tier slots and SSD extents) so that they can be reused.
- **SC-004**: The dispatcher correctly handles concurrent populate and lookup operations on different keys without data corruption or deadlock.
- **SC-005**: Initialization fails gracefully with a descriptive error when required dependencies (dispatch_map, memory_tier) are not bound.
- **SC-006**: Shutdown drains all pending background write-through jobs before returning, ensuring data consistency.
- **SC-007**: The dispatcher supports N independent data block devices and extent managers operating in parallel, with key-based drive selection.
- **SC-008**: ~~REMOVED~~ *(superseded 2026-07-02)* — The prepare_store/commit_store workflow has been removed.
- **SC-009**: Capacity-based eviction correctly removes LRU entries from the memory-tier when the pool is full, transitioning them to BlockDevice state.
- **SC-010**: The touch operation refreshes an entry's dispatch-map timestamp and memory-tier LRU position without performing DMA.
- **SC-011**: Entries evicted from memory-tier but present on SSD can be promoted back via the pipelined reader on subsequent lookup.
- **SC-012**: The pipelined reader correctly transfers MDTS-sized chunks from SSD to both memory-tier and GPU using the sliding-window approach (overlapping NVMe reads with GPU H2D copies via FIFO queue pair), producing correct data under the FIFO NVMe completion ordering assumption.
- **SC-013**: The background SSD evictor removes the oldest BlockDevice entries when SSD utilization exceeds the configured threshold, freeing extents until utilization drops below the low-water mark.
- **SC-014**: `batch_lookup` with a batch of 20 cold entries across 3 drives completes with total wall time bounded by the slowest single drive (parallel promotion), achieving ≥5x throughput improvement over sequential single-entry lookups.
- **SC-015**: Concurrent `batch_lookup` promotions of the same cold key both succeed — the loser of the `IMemoryTier::insert` race is served the winner's promoted data (via the concurrent-promotion recovery pass) rather than failing the load with `AlreadyExists`.
- **SC-016** *(Sync 2026-08-20 — backfilled; covers FR-057)*: With mock block-device and extent-manager factories injected via `set_block_device_factory` / `set_extent_manager_factory` and a `PipelineMetrics` recorder installed via `set_pipeline_metrics`, a test can exercise dispatcher data-path operations and observe their pipeline timings without SPDK/NVMe hardware; when no factory is injected the dispatcher uses its default hard-coded implementations.

## Assumptions

- Clients provide valid IPC handles referencing accessible GPU memory regions. The dispatcher does not validate GPU memory accessibility.
- The SPDK environment is initialized and active before the dispatcher's `initialize()` is called (via the ISPDKEnv receptacle). When ISPDKEnv is not connected, the dispatcher operates in memory-tier-only mode.
- The memory-tier pool is pre-allocated before initialization. The dispatcher does not manage the pool lifecycle (only calls insert/get/peek/remove/evict_next/oldest_keys/touch).
- DMA buffer allocation for the pipelined reader uses SPDK DMA allocation (falls back to libc `aligned_alloc` without SPDK).
- GPU-to-host DMA transfers use `IGpuServices::dma_copy_to_host` (populate direction). Host-to-GPU DMA transfers use `IGpuServices::memcpy_h2d_async` on the warm stream (raw pointer path for memory-tier hits) or `dma_copy_to_device_async` (DmaBuffer path for pipeline/promotion), falling back to synchronous `dma_copy_to_device` when streams are unavailable (lookup direction).
- NVMe SSDs have a Maximum Data Transfer Size (MDTS) limit. The `io_segmenter` module provides MDTS-aware I/O splitting.
- Memory-tier pointers are wrapped in DmaBuffer with a `noop_free` function -- the memory-tier component owns the memory; the DmaBuffer wrapper must not free it.
- Write-through is best-effort: failure does not propagate to the caller. Data remains accessible from the memory-tier until eviction.
- The background writer holds a read reference on each entry while write-through is in progress. The reference is released after write completes or fails.
- Block devices and extent managers are created internally during `initialize()` -- callers provide PCI BDF address strings, not pre-constructed components.
