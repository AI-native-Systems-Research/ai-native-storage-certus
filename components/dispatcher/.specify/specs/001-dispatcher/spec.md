# Feature Specification: Dispatcher Component

**Feature Branch**: `001-dispatcher`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The Dispatcher is the central data-plane orchestrator for the Certus storage system. It implements the `IDispatcher` interface to coordinate GPU-to-SSD cache operations (populate, lookup, check, remove, touch) through a DRAM memory-tier with LRU eviction and write-through persistence to NVMe SSDs. The component manages N data block devices with N extent managers for striped persistent storage.

Data enters via GPU IPC handles, lands in a CUDA-pinned DRAM pool managed by `IMemoryTier`, and is asynchronously written through to SSD by a per-drive parallel background writer. Cache lookups that hit the memory-tier are served via async H2D DMA (~10 GB/s from pinned memory). Cold lookups promote SSD-resident entries back into the memory-tier using a zero-copy pipelined reader that overlaps NVMe reads with GPU DMA transfers across dual CUDA streams. A persistent cold-path worker pool pre-allocates NVMe queue pairs and CUDA streams to eliminate per-batch connection setup overhead.

## User Scenarios & Testing

### User Story 1 - Cache Populate from GPU (Priority: P1)

As a GPU inference application, I want to offload KV-cache tensors from GPU VRAM to persistent storage, so that GPU memory can be reclaimed for active inference while the tensor remains retrievable.

**Acceptance Scenarios**:

- **Given** an initialized dispatcher with bound receptacles, **When** `populate(key, ipc_handle)` is called with a valid non-zero-size IPC handle, **Then** the data is DMA-copied from GPU to a memory-tier slot, a dispatch-map entry is created, and a background write-through job is enqueued for SSD persistence.
- **Given** a full memory-tier pool, **When** `populate` is called, **Then** LRU entries are evicted (up to `max_eviction_attempts`) to make space before allocating.
- **Given** a key that already exists, **When** `populate` is called with that key, **Then** `AlreadyExists` error is returned.
- **Given** `ipc_handle.size == 0`, **When** `populate` is called, **Then** `InvalidParameter` error is returned.

### User Story 2 - Hot Cache Lookup (Memory-Tier Hit) (Priority: P1)

As a GPU inference application, I want to retrieve a previously cached tensor directly from DRAM to GPU, so that retrieval latency is minimal (~microseconds).

**Acceptance Scenarios**:

- **Given** a key whose entry is in the memory-tier, **When** `lookup(key, ipc_handle)` is called, **Then** data is DMA-copied H2D from the memory-tier slot to the GPU destination using async multi-stream transfers, and the entry's LRU timestamp is refreshed.
- **Given** a key that does not exist, **When** `lookup` is called, **Then** `KeyNotFound` error is returned.

### User Story 3 - Cold Cache Lookup (SSD Promotion) (Priority: P1)

As a GPU inference application, I want to retrieve a tensor that has been evicted from DRAM to SSD, so that cold data remains accessible with acceptable latency.

**Acceptance Scenarios**:

- **Given** a key whose entry is on SSD (BlockDevice state), **When** `lookup(key, ipc_handle)` is called, **Then** a memory-tier slot is allocated (evicting if needed), NVMe reads are pipelined directly into the slot (zero-copy), async H2D DMA streams data to GPU overlapped with reads, and the dispatch-map is updated to MemoryTier state.
- **Given** a batch of cold entries across multiple drives, **When** `batch_lookup` is called, **Then** entries are promoted in parallel (up to 2 queue threads per drive) to exploit multi-drive bandwidth.

### User Story 4 - Remote Lookup Fallback (Priority: P2)

As a multi-node Certus deployment, I want local cache misses to be forwarded to remote nodes, so that data cached on peer nodes can be retrieved without re-computation.

**Acceptance Scenarios**:

- **Given** an `IRemoteLookup` receptacle is connected and a key is not found locally, **When** `batch_lookup` is called, **Then** the dispatcher forwards the missed keys to the remote lookup service and returns the remote results.

### User Story 5 - Cache Entry Removal (Priority: P1)

As a cache manager, I want to explicitly remove entries from all tiers, so that invalidated data does not consume resources.

**Acceptance Scenarios**:

- **Given** a key exists in the cache (any tier), **When** `remove(key)` is called, **Then** the entry is removed from memory-tier, dispatch-map, and SSD extent (if applicable).
- **Given** a key that does not exist, **When** `remove` is called, **Then** `KeyNotFound` error is returned.

### User Story 6 - Three-Phase Memory Reservation (Priority: P1)

As a high-throughput KV-cache offloading pipeline, I want to separate memory reservation from data copy and registration, so that I can overlap GPU DMA with other work (backpressure hotpath).

**Acceptance Scenarios**:

- **Given** an initialized dispatcher, **When** `reserve_memory(key, size)` is called, **Then** a DRAM slot is allocated and its pointer returned, but the key is NOT visible via `check()`.
- **Given** a reserved slot, **When** `copy_gpu_to_memory_async(key, handle, stream)` is called, **Then** GPU data is async-copied into the slot on the specified stream.
- **Given** the stream is synchronized, **When** `copy_gpu_to_memory_completed(key, size)` is called, **Then** the key is registered in the dispatch-map and becomes visible via `check()`, and a background SSD write-through is enqueued.
- **Given** a reserved slot, **When** `release_memory(key)` is called, **Then** the slot is freed without registering (cancellation path). This is idempotent.

### User Story 7 - Background Write-Through (Priority: P1)

As the storage subsystem, I want populated entries to be asynchronously persisted to NVMe SSDs, so that entries survive DRAM eviction without blocking the populate path.

**Acceptance Scenarios**:

- **Given** entries have been populated, **When** the background writer thread processes jobs, **Then** data is written from memory-tier to SSD via MDTS-aware segmented I/O and the dispatch-map entry transitions to include an `ssd_offset`.
- **Given** `flush_to_ssd()` is called, **Then** the call blocks until all enqueued write jobs complete and returns the count of flushed entries.
- **Given** `shutdown()` is called, **Then** all pending background writes drain before the dispatcher exits.

### User Story 8 - SSD Background Eviction (Priority: P2)

As the storage subsystem, I want SSD space to be reclaimed when utilization exceeds a threshold, so that the system does not run out of persistent storage.

**Acceptance Scenarios**:

- **Given** SSD utilization exceeds `ssd_eviction_threshold`, **When** the background evictor wakes, **Then** it removes oldest BlockDevice entries from the dispatch-map and frees their extents until utilization drops below `ssd_eviction_low_watermark`.
- **Given** SSD utilization is below threshold, **When** the evictor wakes, **Then** no eviction occurs.

### User Story 9 - Lifecycle and Initialization (Priority: P1)

As a system integrator, I want clear lifecycle semantics, so that I can safely wire, initialize, use, and shut down the component.

**Acceptance Scenarios**:

- **Given** a freshly created `DispatcherComponent`, **When** any operation is called before `initialize()`, **Then** `NotInitialized` error is returned.
- **Given** `initialize(config)` is called with `data_pci_addrs = []`, **Then** `InvalidParameter` error is returned.
- **Given** a successful `shutdown()`, **When** operations are called afterward, **Then** `NotInitialized` error is returned.
- **Given** a component has been shut down, **When** `initialize()` is called again, **Then** the component can be reused (re-initialization).
- **Given** `format_on_init = false`, **When** `initialize()` completes, **Then** the dispatch-map is rebuilt from on-disk extent metadata (crash recovery).

### User Story 10 - Memory-Tier Clearing (Priority: P2)

As a system operator, I want to flush all memory-tier entries to SSD-only state, so that DRAM can be reclaimed for other purposes.

**Acceptance Scenarios**:

- **Given** entries exist in the memory-tier, **When** `clear_memory_tier()` is called, **Then** all entries are evicted via LRU, entries with `ssd_offset` are converted to BlockDevice state, entries without are removed entirely, and the count of cleared entries is returned.

## Requirements

### Functional Requirements

- **FR-001**: The dispatcher MUST implement the `IDispatcher` interface as defined in `interfaces::idispatcher`.
- **FR-002**: `populate(key, ipc_handle)` MUST allocate a memory-tier slot, DMA-copy from GPU to DRAM, register in dispatch-map, and enqueue a background SSD write-through job.
- **FR-003**: `populate` MUST reject zero-size IPC handles with `InvalidParameter`.
- **FR-004**: `populate` MUST reject duplicate keys with `AlreadyExists`.
- **FR-005**: `lookup(key, ipc_handle)` MUST serve memory-tier hits via async H2D DMA and synchronize the stream before returning.
- **FR-006**: `lookup` for SSD-resident entries MUST promote data through the zero-copy pipeline: NVMe reads into memory-tier slot, async H2D DMA to GPU.
- **FR-007**: `lookup_async` MUST return the CUDA stream used for H2D copy without blocking on synchronization.
- **FR-008**: `batch_lookup` MUST classify entries into hot (memory-tier) and cold (SSD), process hot entries inline with multi-stream DMA, and promote cold entries in parallel across drives.
- **FR-009**: `batch_lookup` MUST forward locally-missed keys to `IRemoteLookup` when the receptacle is connected.
- **FR-010**: `check(key)` MUST return `true` if the key exists in any tier, `false` otherwise.
- **FR-011**: `remove(key)` MUST remove the entry from memory-tier, dispatch-map, and free the SSD extent if applicable.
- **FR-012**: `touch(key)` MUST refresh the LRU timestamp in both dispatch-map and memory-tier.
- **FR-013**: `reserve_memory(key, size)` MUST allocate a DRAM slot WITHOUT registering in the dispatch-map.
- **FR-014**: `copy_gpu_to_memory_async(key, ipc_handle, stream)` MUST issue async DMA from GPU source to the reserved DRAM slot on the given stream.
- **FR-015**: `copy_gpu_to_memory_completed(key, size)` MUST register the key in the dispatch-map and enqueue a background write-through job.
- **FR-016**: `release_memory(key)` MUST free the reserved slot; MUST be idempotent (absent key returns Ok).
- **FR-017**: `promote_to_memory_tier(keys)` MUST read SSD-resident entries into memory-tier slots using pipelined reads, update dispatch-map state, and silently skip missing or already-hot keys.
- **FR-018**: `clear_memory_tier()` MUST evict all memory-tier entries via LRU, converting those with `ssd_offset` to BlockDevice and removing those without.
- **FR-019**: `flush_to_ssd()` MUST block until all enqueued background write-through jobs are complete.
- **FR-020**: `initialize(config)` MUST validate that `data_pci_addrs` is non-empty, create N block devices and N extent managers, start background writer threads, and optionally start the SSD evictor.
- **FR-021**: `initialize` with `format_on_init = false` MUST rebuild the dispatch-map from on-disk extent metadata.
- **FR-022**: `shutdown()` MUST drain background writers, checkpoint extent managers, unregister memory-tier from CUDA/SPDK, and shut down all block device actors in two-phase order (signal-all then join-all then detach).
- **FR-023**: All operations (except `shutdown`) MUST return `NotInitialized` if called before `initialize()` or after `shutdown()`.
- **FR-024**: The dispatcher MUST support configurable block device and extent manager factories via `set_block_device_factory` and `set_extent_manager_factory`.
- **FR-025**: The dispatcher MUST distribute keys across drives using a deterministic hash function (splitmix64 finalizer).
- **FR-026**: Memory-tier eviction MUST alternate between targeted LRU (same-shard) and small-batch candidate scanning (every 8th attempt) with a configurable `max_eviction_attempts` bound.
- **FR-027**: The cold-path pipeline MUST use a persistent `ColdReadPool` with pre-connected NVMe channels and CUDA streams per worker, falling back to scoped-thread inline execution if pool creation fails.
- **FR-028**: The background SSD evictor MUST monitor extent-manager utilization at configurable intervals and evict oldest BlockDevice entries until utilization drops below the low-watermark.
- **FR-029**: `initialize` MUST compute per-drive CPU assignments either from `poller_base_cpu` (sequential) or from NUMA topology (round-robin per node).
- **FR-030**: `initialize` MUST set up disk partition tables (metadata + extended metadata + data partitions) on each drive and configure extent managers with partition offsets.

### Non-Functional Requirements

- **NFR-001**: Hot-path lookups (memory-tier hit) MUST achieve throughput limited only by GPU H2D DMA bandwidth (~10 GB/s from pinned memory).
- **NFR-002**: Cold-path lookups MUST use zero-copy pipelined NVMe-to-GPU transfers with configurable NVMe queue depth (default 16 in-flight reads, 128 for batch).
- **NFR-003**: The dispatcher MUST be thread-safe: all public methods are safe to call concurrently from multiple threads.
- **NFR-004**: I/O operations MUST respect the NVMe Maximum Data Transfer Size (MDTS) via the `io_segmenter` module (default 128 KiB segments).
- **NFR-005**: GPU DMA MUST use dual CUDA streams with periodic synchronization (every 8 completions) to bound GPU-side queue depth.
- **NFR-006**: Background write-through MUST use one dedicated thread per NVMe drive (`ParallelBackgroundWriter`) to enable concurrent persistence across drives.
- **NFR-007**: The memory-tier pool MUST be co-registered with both SPDK (`spdk_mem_register`) and CUDA (`cudaHostRegister`) for zero-copy NVMe and GPU DMA.
- **NFR-008**: The component MUST support operation without hardware (staging-only mode) when `ISPDKEnv` is not connected, enabling mock-based unit testing.
- **NFR-009**: Pipeline metrics MUST be reportable via the `PipelineMetrics` trait without coupling the dispatcher to a specific telemetry library.
- **NFR-010**: NVMe read operations MUST have a configurable timeout (5000ms default) with `Timeout` completion handling.
- **NFR-011**: Shutdown MUST use two-phase block device teardown (signal-all, join-all, detach-all) to prevent use-after-free on SPDK transport memory.
- **NFR-012**: The dispatcher crate MUST be buildable without SPDK dependencies when `spdk-backend` feature is disabled.

## Key Entities

| Entity | Description |
|--------|-------------|
| `DispatcherComponent` | Main component implementing `IDispatcher`. Holds receptacles, data drives, background workers, and pipeline state. |
| `DataDrive` | Internal struct holding one (block-device, extent-manager, cached-channels) tuple per NVMe drive. |
| `ParallelBackgroundWriter` | Pool of per-drive writer threads that persist memory-tier entries to SSD. |
| `BackgroundEvictor` | Background thread that monitors SSD utilization and reclaims space by removing oldest entries. |
| `ColdReadPool` | Persistent worker pool with pre-connected NVMe channels and CUDA streams for cold-path pipeline execution. |
| `PipelineRing` | Pre-allocated dual CUDA streams and chunk size for pipelined SSD-to-GPU transfers. |
| `WriteJob` | Message sent to background writer: (key, size, device_index). |
| `ColdReadJob` | Describes a single object to pipeline from SSD to GPU: (mem_ptr, gpu_dst, start_lba, total_bytes). |
| `ColdReadRequest` | Request submitted to cold pool: batch of ColdReadJobs + configuration. |
| `IoSegment` | One MDTS-sized chunk of a larger transfer: (buffer_offset, lba, length). |
| `DispatcherConfig` | Configuration struct with PCI addresses, eviction thresholds, partition sizes, and CPU pinning. |
| `DispatcherError` | Error enum: NotInitialized, KeyNotFound, AlreadyExists, AllocationFailed, IoError, Timeout, InvalidParameter. |
| `IpcHandle` | GPU memory handle: (address pointer, size in bytes). |
| `BlockDeviceFactory` | Pluggable factory for creating block device components (DI for testing). |
| `ExtentManagerFactory` | Pluggable factory for creating extent manager components (DI for testing). |
| `PipelineMetrics` | Trait for external instrumentation of pipeline stage timings. |

## Dependencies

| Dependency | Interface | Direction | Purpose |
|-----------|-----------|-----------|---------|
| `ILogger` | receptacle | inbound | Structured logging (info/warn/error/debug) |
| `IDispatchMap` | receptacle | inbound | Key-to-location mapping with reference counting |
| `IGpuServices` | receptacle | inbound | GPU DMA copies, CUDA stream management, memory registration |
| `ISPDKEnv` | receptacle | inbound | SPDK environment for NVMe device discovery and memory management |
| `IMemoryTier` | receptacle | inbound | DRAM pool management with LRU eviction |
| `IRemoteLookup` | receptacle | inbound (optional) | Forwarding local misses to remote Certus nodes |
| `IBlockDevice` | internal | created | NVMe block device driver (per data drive) |
| `IBlockDeviceAdmin` | internal | created | Block device lifecycle management (init, shutdown, detach) |
| `IExtentManager` | internal | created | Fixed-size extent allocation with crash-consistent metadata |
| `DiskPartitionManager` | internal | created | GPT partition table management per drive |
| `component-framework` | build | - | Component model macros and infrastructure |
| `interfaces` | build | - | Shared interface trait definitions (with `spdk` feature) |
| `crossbeam-channel` | build | - | Lock-free MPMC channels for background writer communication |
| `parking_lot` | build | - | Efficient RwLock for data drive vector and pipeline ring |

## Success Criteria

1. All unit tests pass (`cargo test -p dispatcher`) without hardware.
2. Lazy migration tests verify entries transition from MemoryTier to BlockDevice state after background writer drains.
3. Reserve-memory tests verify the three-phase lifecycle (reserve, copy-async, completed) correctly registers entries.
4. Eviction tests verify pool pressure triggers LRU eviction with bounded attempts.
5. Hardware integration tests pass with real NVMe devices and SPDK (`--features hardware-test`).
6. Criterion benchmarks demonstrate expected throughput for mock-based and hardware paths.
7. Formally verified properties (P1-P10) hold as documented in `components/dispatcher/verif/`.
8. Component operates correctly in staging-only mode (no SPDK) for development and CI.
9. Shutdown completes cleanly: all background threads exit, extent managers checkpoint, and no resource leaks.
10. Concurrent operations from multiple threads do not cause data races or panics.

## Implementation Notes

- **Component wiring**: `define_component!` macro generates `DispatcherComponent` with receptacles for `ILogger`, `IDispatchMap`, `IGpuServices`, `ISPDKEnv`, `IMemoryTier`, and `IRemoteLookup`.
- **Drive selection**: Uses splitmix64 finalizer hash for uniform key-to-drive distribution.
- **Memory management**: `DmaBuffer` wrappers around memory-tier pointers use `noop_free` to prevent the dispatcher from freeing memory owned by the memory-tier component.
- **Two-phase shutdown**: Block device actors are signaled to stop first (phase 1), then all threads are joined (phase 2), then controllers are detached (phase 3). This prevents use-after-free when SPDK transport teardown invalidates memory.
- **Eviction strategy**: Alternates between targeted same-shard LRU eviction (fast O(1) path) and small-batch candidate scanning (every 8th attempt) to balance between speed and finding cleanly-evictable entries.
- **Cold pool**: Pre-connected workers with bounded(1) channels provide backpressure. Falls back to inline scoped-thread execution if pool creation fails.
- **Partition layout**: Each drive gets three GPT partitions: metadata (128 MiB default), extended metadata (128 MiB default), and data (remainder). Extent managers are configured with partition-relative LBA offsets.
- **Feature gates**: `spdk-backend` feature enables the real SPDK NVMe block device. `hardware-test` feature enables integration tests requiring real hardware. `pipeline-telemetry` feature enables stderr timing output for debugging.
