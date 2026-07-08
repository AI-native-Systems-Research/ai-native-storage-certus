# Feature Specification: Dispatcher-P2P (GPUDirect Cold-Path Dispatcher)

**Feature Branch**: `001-dispatcher-p2p`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `dispatcher-p2p` component is a GPU-direct peer-to-peer variant of the Certus dispatcher that implements the `IDispatcher` interface. It manages the lifecycle of cache entries across a three-tier storage hierarchy: GPU VRAM (client), DRAM memory-tier, and NVMe SSDs. Its distinguishing feature is the P2P cold-read path: when data must be read from SSD to serve a GPU client, it bypasses host DRAM entirely by DMA-ing NVMe data into pre-allocated GPU BAR1 staging buffers and then performing a device-to-device copy to the client's final GPU destination.

The component orchestrates multiple background subsystems including parallel write-through to SSD, DRAM backfill after P2P cold reads, SSD capacity eviction, and memory-tier pressure eviction. It supports multi-drive configurations with NUMA-aware CPU pinning, partition table management, and crash-consistent extent allocation via integrated block-device and extent-manager components.

## User Scenarios & Testing

### User Story 1 - Hot Lookup from Memory-Tier (Priority: P1)

As a GPU inference client, I want to retrieve a cached tensor from the memory-tier with minimal latency, so that my inference pipeline is not stalled waiting for data.

**Acceptance Scenarios**:
- Given a key exists in the memory-tier with valid data
- When `lookup(key, ipc_handle)` is called
- Then the data is copied from memory-tier DRAM to the client's GPU destination via H2D DMA, and `Ok(())` is returned

- Given a key exists in the memory-tier
- When `lookup_async(key, ipc_handle)` is called
- Then the copy is issued asynchronously on the warm stream and the stream handle is returned for caller synchronization

### User Story 2 - Cold Lookup via P2P Path (Priority: P1)

As a GPU inference client, I want cold data reads from SSD to reach my GPU with minimal latency by avoiding the DRAM bounce, so that cold-path tail latency is reduced.

**Acceptance Scenarios**:
- Given a key exists only on SSD (BlockDevice state) and the P2P ring is initialized
- When `lookup(key, ipc_handle)` is called
- Then data is read from NVMe directly into GPU BAR1 staging slots, D2D-copied to the client destination, the key remains in BlockDevice state, and a DRAM backfill job is enqueued asynchronously

- Given the P2P ring is NOT available (GDRCopy unavailable)
- When a cold lookup occurs
- Then the DRAM fallback path is used: NVMe reads into a memory-tier slot, then H2D to GPU

### User Story 3 - Populate Cache from GPU (Priority: P1)

As a data pipeline client, I want to store a GPU-resident tensor into the cache so that future inference requests can retrieve it without re-computation.

**Acceptance Scenarios**:
- Given the dispatcher is initialized and memory-tier has available space
- When `populate(key, ipc_handle)` is called with size > 0
- Then GPU data is DMA'd to a memory-tier slot, the entry is registered in the dispatch-map, and a background write-through job is enqueued to persist it to SSD

- Given the key already exists in the cache
- When `populate(key, ipc_handle)` is called
- Then `Err(DispatcherError::AlreadyExists(key))` is returned

- Given size is 0
- When `populate(key, ipc_handle)` is called
- Then `Err(DispatcherError::InvalidParameter)` is returned

### User Story 4 - Batch Lookup for Multi-Object Inference (Priority: P1)

As a GPU inference client performing batched inference, I want to look up multiple cache entries in a single call so that I/O is overlapped and total latency is reduced.

**Acceptance Scenarios**:
- Given a batch of N keys, some in memory-tier and some on SSD
- When `batch_lookup(entries)` is called
- Then hot entries are served immediately via H2D DMA, cold entries are grouped by drive and served in parallel via P2P pipeline, and a Vec of per-entry results is returned

- Given some keys do not exist locally but a remote_lookup receptacle is bound
- When `batch_lookup` encounters KeyNotFound results
- Then those keys are forwarded to the remote_lookup service for distributed cache resolution

### User Story 5 - Initialization with Drive Discovery (Priority: P1)

As a system operator, I want the dispatcher to discover and initialize NVMe drives by PCI address, create partition tables, format extent managers, and set up the P2P staging ring at startup.

**Acceptance Scenarios**:
- Given valid PCI addresses, bound receptacles, and GPU with BAR1 support
- When `initialize(config)` is called
- Then block devices are created and initialized per PCI address, partition tables are created/recovered, extent managers are formatted/recovered, the P2P ring is allocated, the pipeline ring is allocated, memory-tier is registered for CUDA+SPDK DMA, background workers are started, and the component transitions to initialized state

- Given format_on_init is false and extent managers have existing data
- When `initialize(config)` is called
- Then the dispatch-map is rebuilt by iterating all extents on all drives (crash recovery)

### User Story 6 - Graceful Shutdown (Priority: P1)

As a system operator, I want the dispatcher to cleanly shut down all background workers, checkpoint metadata, and release hardware resources.

**Acceptance Scenarios**:
- Given the dispatcher is initialized with active background workers and drives
- When `shutdown()` is called
- Then the SSD evictor is stopped, background writers drain remaining jobs, DRAM backfill workers stop, the cold-pool is shut down, extent managers are checkpointed, memory-tier pool is unregistered from CUDA/SPDK, CUDA streams are destroyed, P2P ring is destroyed, block device actors are signal-stopped then joined then detached (three-phase), remote lookup leaves cluster, and the component transitions to uninitialized state

### User Story 7 - Memory-Tier Eviction Under Pressure (Priority: P2)

As the system, I want to automatically evict entries from the memory-tier when it is full, so that new entries can be inserted without allocation failures.

**Acceptance Scenarios**:
- Given memory-tier is at capacity and a new entry needs insertion
- When `evict_for_space` is invoked internally
- Then LRU entries with completed SSD write-through are converted from MemoryTier to BlockDevice state in the dispatch-map, freeing their DRAM slots

- Given eviction attempts exceed MAX_ATTEMPTS (512) without freeing sufficient space
- Then `Err(DispatcherError::AllocationFailed)` is returned and a warning is emitted once

### User Story 8 - SSD Capacity Eviction (Priority: P2)

As the system, I want a background evictor to remove cold SSD-only entries when disk utilization exceeds a threshold, so that the SSD does not fill up.

**Acceptance Scenarios**:
- Given SSD utilization exceeds `ssd_eviction_threshold` (e.g., 90%)
- When the background evictor wakes on its polling interval
- Then the oldest BlockDevice-only entries (not in memory-tier) are removed from dispatch-map and their extents freed until utilization drops below `ssd_eviction_low_watermark`

### User Story 9 - DRAM Backfill After P2P Cold Read (Priority: P2)

As the system, I want to asynchronously backfill the memory-tier with data after a P2P cold read, so that subsequent lookups for the same key can be served from DRAM (hot path) rather than SSD (cold path).

**Acceptance Scenarios**:
- Given a P2P cold read has served a client and the backfill delay has elapsed
- When the DRAM backfill worker processes the job
- Then the data is read from SSD into a memory-tier slot and the dispatch-map entry is converted from BlockDevice to MemoryTier state

### User Story 10 - Check Key Existence (Priority: P2)

As a client, I want to check if a key exists in the cache without transferring data, so that I can make scheduling decisions.

**Acceptance Scenarios**:
- Given a key exists (in any state)
- When `check(key)` is called
- Then `Ok(true)` is returned

- Given a key does not exist
- When `check(key)` is called
- Then `Ok(false)` is returned

### User Story 11 - Remove Cache Entry (Priority: P2)

As a client, I want to remove a cache entry by key, freeing both memory-tier and SSD resources.

**Acceptance Scenarios**:
- Given a key exists in memory-tier and/or on SSD
- When `remove(key)` is called
- Then the entry is removed from memory-tier (if present), removed from dispatch-map, and its SSD extent is freed (if it had been written through)

- Given a key does not exist
- When `remove(key)` is called
- Then `Err(DispatcherError::KeyNotFound)` is returned

### User Story 12 - Pre-warm Memory Tier (Priority: P3)

As a system operator or prefetch heuristic, I want to promote a set of SSD-resident keys into the memory-tier proactively, so that subsequent client lookups are served from the hot path.

**Acceptance Scenarios**:
- Given a list of keys in BlockDevice state
- When `promote_to_memory_tier(keys)` is called
- Then entries are read from SSD into memory-tier in parallel (grouped by drive), and converted to MemoryTier+Storage state in the dispatch-map

### User Story 13 - Clear Memory Tier (Priority: P3)

As a system operator, I want to evict all entries from the memory-tier to free DRAM, converting them to SSD-only state.

**Acceptance Scenarios**:
- Given entries exist in the memory-tier
- When `clear_memory_tier()` is called
- Then all entries are evicted via LRU, converted to BlockDevice state, and the count of evicted entries is returned

### User Story 14 - Flush Pending Writes to SSD (Priority: P3)

As a system operator performing a checkpoint, I want to block until all pending write-through jobs have completed.

**Acceptance Scenarios**:
- Given background write jobs are in-flight
- When `flush_to_ssd()` is called
- Then it blocks until all queued jobs are processed and returns the count of in-flight jobs at call time

## Requirements

### Functional Requirements

- **FR-001**: The component MUST implement the `IDispatcher` interface as defined in `components/interfaces/`.
- **FR-002**: The component MUST provide two cold-read data paths: (a) P2P path (NVMe -> GPU BAR1 -> D2D -> client GPU) and (b) DRAM fallback path (NVMe -> DRAM -> H2D -> client GPU). Path selection MUST be determined at initialization based on hardware capability.
- **FR-003**: The hot path MUST copy data from memory-tier DRAM to client GPU via H2D DMA (memcpy_h2d_async when warm stream available, dma_copy_to_device as fallback).
- **FR-004**: The component MUST distribute data across N NVMe drives using a deterministic hash function (splitmix64 finalizer on the cache key modulo drive count).
- **FR-005**: The component MUST manage disk partition tables (metadata partition, extended metadata partition, data partition) via `DiskPartitionManager` for each drive.
- **FR-006**: The component MUST support both format-on-init (fresh start) and recovery (rebuild dispatch-map from existing extents) initialization modes.
- **FR-007**: Background write-through MUST persist memory-tier entries to SSD asynchronously using one dedicated writer thread per drive.
- **FR-008**: DRAM backfill MUST be performed asynchronously after P2P cold reads, with a configurable delay (`backfill_delay_ms`).
- **FR-009**: Background SSD eviction MUST be triggered when aggregate extent-manager utilization exceeds `ssd_eviction_threshold` and MUST stop when utilization falls below `ssd_eviction_low_watermark`.
- **FR-010**: Memory-tier eviction MUST use a combination of targeted eviction (oldest keys with ssd_offset from dispatch-map) and LRU eviction (via memory-tier's evict_lru_for_key) to free space for new insertions.
- **FR-011**: All IDispatcher methods (except `shutdown`) MUST return `DispatcherError::NotInitialized` if called before `initialize()` or after `shutdown()`.
- **FR-012**: `populate` MUST reject zero-size IPC handles with `DispatcherError::InvalidParameter`.
- **FR-013**: `populate` MUST return `DispatcherError::AlreadyExists` for duplicate keys.
- **FR-014**: `batch_lookup` MUST forward locally-not-found keys to the `remote_lookup` receptacle (if bound) for distributed cache resolution.
- **FR-015**: `shutdown` MUST use three-phase block device teardown: (1) signal all actors to stop, (2) join all actor threads, (3) detach controllers. This prevents crashes from SPDK transport teardown invalidating memory that actors are still polling.
- **FR-016**: `shutdown` MUST checkpoint all extent managers to persist metadata before teardown.
- **FR-017**: The component MUST support factory-based block device and extent manager creation via `set_block_device_factory` and `set_extent_manager_factory` for testability and alternate backends.
- **FR-018**: NUMA-aware CPU pinning MUST be applied to NVMe poller threads when `poller_base_cpu` is not explicitly configured, using SPDK device topology and round-robin allocation from the device's NUMA node CPUs.
- **FR-019**: `batch_lookup` MUST serve cold entries in parallel using one worker per drive via the P2P cold-read pool (or inline fallback if pool is unavailable).
- **FR-020**: The component MUST register the memory-tier pool with CUDA (`cudaHostRegister`) and SPDK (`spdk_mem_register`) for zero-copy NVMe reads and async GPU transfers.
- **FR-021**: `remove` MUST free the SSD extent (via extent_mgr.remove_extent) when the entry had been written through to disk.
- **FR-022**: `touch(key)` MUST update the LRU position of an entry in the dispatch-map.
- **FR-023**: `clear_memory_tier` MUST evict all entries from memory-tier via LRU and convert each to BlockDevice state.
- **FR-024**: `flush_to_ssd` MUST block until all per-drive background writer queues are drained.
- **FR-025**: The P2P ring MUST be partitioned across threads to provide lock-free concurrent access. Each partition provides up to MAX_QD_PER_THREAD (16) slots.
- **FR-026**: `reserve_memory`, `copy_gpu_to_memory_async`, and `copy_gpu_to_memory_completed` MUST support the three-phase populate protocol (reserve slot, async DMA, register entry + enqueue write-through).
- **FR-027**: `release_memory` MUST be idempotent (KeyNotFound is not an error).
- **FR-028**: The `promote_to_memory_tier` method MUST process entries in parallel (scoped threads, one per drive).

### Non-Functional Requirements

- **NFR-001**: The hot path (memory-tier to GPU) MUST NOT allocate memory on the critical path; it uses pre-allocated warm streams and noop-free DmaBuffer wrappers.
- **NFR-002**: The cold-read pipeline MUST maintain at least `effective_qd` NVMe commands in flight per drive to saturate PCIe bandwidth.
- **NFR-003**: P2P ring allocation (64 slots x chunk_size) MUST complete in under 1 second at initialization.
- **NFR-004**: The P2P cold-path MUST eliminate the host DRAM bounce, transferring data in one PCIe hop (NVMe -> GPU BAR1) plus one intra-GPU copy (BAR1 -> VRAM).
- **NFR-005**: The component MUST be thread-safe. All shared state is protected by `RwLock`, `Mutex`, or `Atomic` primitives.
- **NFR-006**: Background workers (write-through, backfill, evictor) MUST drain all pending jobs before shutdown completes.
- **NFR-007**: I/O segmentation MUST respect the NVMe device's maximum transfer size (MDTS), splitting large transfers into multiple segments.
- **NFR-008**: The cold-path pipeline MUST use multiple CUDA streams (4 for P2P, 2 for DRAM path) to overlap NVMe I/O with GPU DMA transfers.
- **NFR-009**: Memory-tier eviction MUST bound its scan to MAX_ATTEMPTS (512) iterations to prevent unbounded loops under extreme pressure.
- **NFR-010**: The P2P cold-read pool pre-connects NVMe client channels at init to eliminate per-batch connection overhead.
- **NFR-011**: The component MUST support up to 4 NVMe drives with 1 queue thread per drive, each getting the full 16-slot ring partition.

## Key Entities

| Entity | Description |
|--------|-------------|
| `DispatcherP2pComponent` | Main component struct; implements `IDispatcher`. Fields include receptacles, background worker handles, data drives, pipeline ring, P2P ring. |
| `DataDrive` | Holds one (block-device, extent-manager) pair for a data drive, including cached client channels. |
| `P2pRing` | Pre-allocated ring of 64 GPU BAR1 staging buffers with CUDA streams for D2D copies. Partitioned across threads. |
| `ThreadPartition` | Describes a thread's non-overlapping slice of the P2P ring (offset + effective QD). |
| `PipelineRing` | Pre-allocated ring of 16 CUDA-pinned + SPDK-registered DMA buffers and 2 CUDA streams for the DRAM-path pipeline. |
| `P2pColdReadPool` | Persistent worker pool with pre-connected NVMe channels for cold-path pipeline execution. |
| `ParallelBackgroundWriter` | Pool of per-drive background writer threads that persist memory-tier entries to SSD. |
| `DramBackfillWorker` | Per-drive background worker that reads SSD data into DRAM memory-tier slots after P2P cold reads. |
| `BackgroundEvictor` | Background thread that evicts cold SSD-only entries when disk utilization exceeds threshold. |
| `IoSegment` | Represents a portion of a larger I/O transfer, respecting MDTS boundaries. |
| `WriteJob` | Describes a background write-through job (key, size, device_index). |
| `DramBackfillJob` | Describes a background DRAM backfill job (key, drive_index). |
| `P2pColdReadRequest` | Work unit submitted to the cold-pool worker (jobs, partition, ring pointer, result channel). |
| `BlockDeviceFactory` | Pluggable factory function for creating block device components. |
| `ExtentManagerFactory` | Pluggable factory function for creating extent manager components. |
| `DispatcherConfig` | Configuration struct passed to `initialize()` containing PCI addresses, partition sizes, eviction thresholds, etc. |

## Dependencies

### Required Receptacles (must be bound before initialize)

| Receptacle | Interface | Purpose |
|------------|-----------|---------|
| `dispatch_map` | `IDispatchMap` | Tracks entry locations (MemoryTier/BlockDevice) with reference counting |
| `memory_tier` | `IMemoryTier` | DRAM slab allocator for cache data |
| `gpu_services` | `IGpuServices` | CUDA DMA operations, stream management, memory registration |
| `spdk_env` | `ISPDKEnv` | SPDK environment for NVMe device enumeration (optional if factory set) |
| `logger` | `ILogger` | Structured logging |
| `remote_lookup` | `IRemoteLookup` | Distributed cache resolution (optional, graceful degradation) |

### Crate Dependencies

| Crate | Purpose |
|-------|---------|
| `component-framework` | Component model macros and traits |
| `interfaces` (spdk feature) | Shared interface definitions |
| `gpu-services` (spdk, gpu, p2p features) | CUDA FFI, GDRCopy BAR1 buffer creation |
| `spdk-env` | SPDK environment wrapper |
| `block-device-spdk-nvme` (optional) | NVMe block device driver |
| `extent-manager` | Fixed-size extent allocator |
| `disk-partition-manager` | GPT partition table management |
| `memory-tier` | DRAM pool management |
| `crossbeam-channel` | Lock-free MPSC channels for background workers |
| `parking_lot` | High-performance RwLock |

### System Requirements

| Requirement | Purpose |
|-------------|---------|
| GPUs with large-BAR enabled in BIOS | BAR1 memory accessible for GDRCopy mapping |
| GDRCopy (`libgdrapi.so`, `gdrdrv` kernel module) | Maps GPU device memory to BAR1 for CPU/DMA access |
| `nvidia-peermem` kernel module | PCIe P2P DMA between NVMe and GPU |
| NVMe drives on same PCIe root complex as GPU | Required for P2P DMA (same-socket topology) |
| SPDK with hugepages and IOMMU | Userspace NVMe driver |

## Success Criteria

1. Hot-path lookup latency is dominated by H2D DMA transfer time (no allocation or locking on critical path).
2. Cold-path P2P lookup eliminates DRAM bounce: data flows NVMe -> GPU BAR1 -> GPU VRAM in a single PCIe hop for the bulk data.
3. Multi-drive configurations achieve linear throughput scaling for cold reads (one pipeline thread per drive).
4. Background write-through ensures all memory-tier entries are persisted to SSD without blocking the populate call.
5. Memory-tier eviction prevents allocation failures under sustained load.
6. SSD eviction prevents disk full conditions under sustained ingestion.
7. Shutdown completes cleanly: no data loss (extent checkpoint), no resource leaks (CUDA streams/buffers freed), no crashes (three-phase actor teardown).
8. The component passes all unit tests including concurrent multi-thread scenarios.

## Implementation Notes

- The P2P ring uses 64 slots total (`P2P_RING_SLOTS`). With 4 drives and `MAX_QUEUES_PER_DRIVE=1`, each thread gets a 16-slot partition. This matches the NVMe queue depth needed to saturate PCIe bandwidth per drive.
- The `noop_free` function is used extensively to wrap memory-tier pointers in `DmaBuffer` without transferring ownership. These wrappers are intentionally `std::mem::forget`'d to avoid double-free.
- PCI address parsing expects format `DDDD:BB:DD.F` (domain:bus:device.function in hex).
- The drive_index function uses splitmix64 finalizer for uniform distribution of keys across drives.
- The warm_stream is stored as `AtomicU64` (cast from pointer) to allow lock-free access on the hot path.
- The DRAM backfill worker introduces a configurable delay (`backfill_delay_ms`) to avoid thrashing when the same key is requested repeatedly in quick succession from the P2P path.
- Memory-tier eviction uses a hybrid strategy: every 8th attempt tries targeted eviction via `oldest_keys` + `is_evictable`, otherwise uses `evict_lru_for_key`.
- The `pipeline-telemetry` feature flag enables per-segment timing breakdowns for performance profiling (disabled in production).
