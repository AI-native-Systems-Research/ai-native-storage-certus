# Feature Specification: GPUDirect Storage Cold Path

**Feature Branch**: `p2p_component`

**Created**: 2026-06-11

**Status**: Draft

**Last-Synced**: 2026-08-20 (Spec-Sync Phase B — SC-006 reworded to match implemented graceful-init/deferred-panic behavior; FR-018..FR-023 backfilled for previously unspecced background/admin/async features)

**Input**: User description: "GPUDirect Storage cold-read path for dispatcher-p2p. NVMe DMA reads directly into GPU BAR1 staging buffers, then D2D copies to client GPU destination, eliminating host DRAM bounce."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Cold Lookup Completes via P2P Path (Priority: P1)

A client application requests data that has been evicted from DRAM to NVMe SSD. The system reads the data from SSD directly into a GPU staging buffer, then copies it to the client's GPU destination without bouncing through host memory.

**Why this priority**: This is the sole reason this component exists. Without a working P2P cold path, the component adds no value over the standard dispatcher.

**Independent Test**: Evict entries from DRAM, issue lookups, verify data arrives correctly at the client GPU destination.

**Acceptance Scenarios**:

1. **Given** an entry evicted from DRAM to SSD, **When** a client requests that entry, **Then** data arrives at the client GPU destination with correct content.
2. **Given** multiple chunks comprising a single entry, **When** the pipelined read completes, **Then** all chunks are present and ordered correctly at the destination.
3. **Given** 4 concurrent clients requesting different cold entries, **When** lookups proceed in parallel, **Then** each client receives its own correct data without corruption.

---

### User Story 2 - Fail Fast When P2P Unavailable (Priority: P2)

When the P2P staging ring cannot be initialized (missing gdrdrv/nvidia-peermem kernel modules, insufficient GPU memory), the server MUST fail at startup rather than silently degrading. Use the `full.yaml` profile (standard dispatcher) for DRAM-only deployments.

**Why this priority**: Silent degradation to DRAM defeats the purpose of selecting the P2P profile. Explicit failure prevents misdiagnosis.

**Independent Test**: Remove gdrdrv module, start server with full-p2p profile, verify it panics during initialization.

**Acceptance Scenarios**:

1. **Given** a system where P2P initialization fails, **When** the component starts, **Then** it logs a diagnostic warning. On the first cold lookup attempt, it panics with a message directing the operator to use the full.yaml profile.
2. **Given** partial resource allocation before failure, **When** initialization fails, **Then** all partially allocated GPU memory is freed before the panic.

---

### User Story 3 - Hot Path Unaffected (Priority: P2)

Lookups for entries still in DRAM proceed exactly as in the standard dispatcher with no performance degradation from P2P machinery.

**Why this priority**: Hot path is the common case. Any regression here would negate the value of the cold path optimization.

**Independent Test**: Measure hot-path lookup throughput with the P2P component vs the standard dispatcher; verify no regression.

**Acceptance Scenarios**:

1. **Given** an entry present in DRAM, **When** a client requests it, **Then** data is delivered at the same throughput as the standard dispatcher.
2. **Given** concurrent hot and cold lookups, **When** cold lookups are in progress, **Then** hot-path lookups are not blocked or delayed.

---

### User Story 4 - Performance Is Measurable (Priority: P3)

The system's end-to-end performance (P2P path vs DRAM path) can be measured using the existing benchmark tool under realistic workloads with hot/cold mixes and multi-client concurrency.

**Why this priority**: Without measurement, there is no basis for evaluating whether the P2P path delivers value.

**Independent Test**: Run the pipelined benchmark with cold entries, observe that throughput numbers are reported for both paths.

**Acceptance Scenarios**:

1. **Given** a deployed system with the P2P path active, **When** the benchmark tool runs a cold-heavy workload, **Then** throughput and latency numbers are reported.
2. **Given** the standard dispatcher (full.yaml) deployed on the same hardware, **When** the same benchmark runs, **Then** comparable throughput numbers are reported for comparison against the P2P path.

---

### User Story 5 - Automatic Tier Capacity Management (Priority: P2)

The system keeps both DRAM and SSD tiers within configured utilization bounds without operator intervention: it demotes cold DRAM entries to SSD, reclaims SSD capacity when full, and persists writes to SSD concurrently across drives. An administrator can also flush the DRAM tier on demand, and callers can pipeline hot lookups on their own CUDA stream.

**Why this priority**: Sustained inferencing workloads outlive any fixed tier size; without automatic demotion, reclamation, and durable write-through the tiers fill and either error or stall. These are the mechanisms that make the cold path sustainable rather than a one-shot demo.

**Independent Test**: Configure low DRAM and SSD watermarks, drive writes past each threshold, and verify demotion/reclamation events fire and utilization returns below the low watermark; call `clear_memory_tier()` and verify all entries are cleared.

**Acceptance Scenarios**:

1. **Given** `memory_tier_eviction_threshold` set and DRAM utilization above it, **When** the memory-tier evictor sweeps, **Then** oldest evictable entries are demoted to SSD (emitting `Demoted` events) until utilization falls below the low watermark.
2. **Given** `ssd_eviction_threshold` set and SSD utilization above it, **When** the SSD evictor sweeps, **Then** oldest entries are removed and their extents freed (emitting `Removed` events) until utilization falls below the low watermark.
3. **Given** write-through enabled across multiple drives, **When** writes are enqueued, **Then** each is routed to its target drive's writer thread and `flush()` blocks until all per-drive queues are drained.
4. **Given** a populated memory tier, **When** `clear_memory_tier()` is called, **Then** every entry is demoted to its SSD copy or force-removed, and the count of cleared entries is returned.
5. **Given** a hot (DRAM-resident) key, **When** a caller invokes `lookup_async()`, **Then** an H2D copy is issued on a warm CUDA stream and the stream is returned without blocking, and the read pin is held until the caller-driven synchronization completes.

---

### Edge Cases

- What happens when all staging ring slots are occupied by in-flight reads? Additional cold reads MUST queue until a slot is recycled.
- What happens when an NVMe read fails mid-pipeline? The affected lookup MUST return an error; the slot MUST be recycled; other in-flight lookups MUST not be affected.
- What happens when the client GPU becomes unreachable during a D2D copy? The error MUST propagate to the requesting client without corrupting ring state.
- What happens under 4+ concurrent clients? The ring MUST be partitioned to prevent conflicts between threads.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST read evicted data from SSD directly into GPU staging buffers, bypassing host DRAM.
- **FR-002**: System MUST copy data from staging buffers to the client's GPU destination.
- **FR-003**: System MUST pre-allocate a fixed ring of 64 GPU staging buffers at initialization via `cudaMalloc` + GDRCopy BAR1 mapping (`gdr_pin_buffer` + `gdr_map`) + `spdk_mem_register`. Each slot's size is dynamically determined from the drive's `max_transfer_size()` (typically 128 KiB MDTS). The ring includes 4 pre-allocated CUDA streams (minimum 2 on constrained hardware). The ring is shared across all cold lookup threads.
- **FR-004**: System MUST partition the staging ring for concurrent thread access using `ThreadPartition` (non-overlapping slot ranges, effective QD capped at 16 per thread to prevent NVMe qpair saturation). With `MAX_QUEUES_PER_DRIVE=1`, the ring is partitioned into one 16-slot region per drive, maximizing per-drive NVMe queue depth.
- **FR-005**: System MUST pipeline SSD reads with D2D GPU copies using FIFO completion ordering. D2D copies are distributed round-robin across 4 CUDA streams for maximum PCIe overlap. Stream synchronization occurs once per ring partition wrap (sync interval = ring_size) to bound GPU queue depth and ensure slots are safe to reuse. A final stream sync is performed after all chunks complete.
- **FR-006**: The `batch_lookup` path MUST panic if the P2P ring was not initialized (GDRCopy unavailable, GPU memory insufficient). Initialization logs a diagnostic warning but does not fail, allowing hot-only testing without P2P hardware. The single-key `lookup()` path does NOT panic — it silently falls back to the DRAM path when the P2P ring is unavailable (for test/staging environments). Use the `full.yaml` profile (standard dispatcher) for production DRAM-only deployments.
- **FR-007**: The P2P ring is allocated once at initialization and is immutable for the component's lifetime. In production (full-p2p profile via `batch_lookup`), the P2P path is always used for cold reads and panics if unavailable. The single-key `lookup()` DRAM fallback path exists for test/staging environments where GDRCopy is unavailable.
- **FR-008**: System MUST implement the same interface as the standard dispatcher, serving as a drop-in replacement. This includes the `IRemoteLookup` fallback for entries missed locally: the dispatcher calls `IRemoteLookup::batch_lookup` with `(key, size)` pairs (not `IpcHandle` — remote-lookup works only in DRAM), and on a successful remote fetch (the value becomes resident in the local memory tier) performs the DRAM→GPU delivery itself using the memory-tier→device copy.
- **FR-009**: System MUST asynchronously promote cold entries to DRAM via a throttled background worker (`DramBackfillWorker`) after serving the client via P2P. The worker re-reads data from SSD into the memory-tier slot, then registers the key as MemoryTier in the dispatch-map. During the backfill window, repeat lookups of the same key use the P2P cold path (correct data, no stale DRAM). The backfill delay is controlled by `backfill_delay_ms` in `DispatcherConfig`.
- **FR-010**: System MUST release all staging resources on shutdown with no leaks.
- **FR-011**: System MUST handle read failures gracefully without corrupting ring state or affecting other in-flight operations.
- **FR-012**: Performance measurement is handled by external benchmarking tools (e.g., `certus-api-bench_v2.py`) rather than built-in hooks, to avoid instrumentation overhead in the production path.
- **FR-013**: System MUST implement `promote_to_memory_tier(keys)` to asynchronously read cold entries from NVMe into the memory-tier without GPU involvement, enabling future lookups to take the hot DRAM→GPU path. This uses the `pipelined_ssd_to_dram_only` pipeline function (one thread per drive, no P2P ring involvement).
- **FR-014**: System MUST support configurable DRAM backfill throttling via `backfill_delay_ms` in `DispatcherConfig`. Default: 10ms. When set to 0, no background DRAM backfill occurs and cold-promoted keys remain as BlockDevice indefinitely (repeat lookups always use P2P). When > 0, the `DramBackfillWorker` sleeps for that duration between jobs to avoid contending with active P2P cold reads for NVMe bandwidth.
- **FR-015**: The component's `IGpuServices` receptacle MUST expose multi-GPU device selection — `set_device(device)` to bind the active CUDA device and `device_of_ptr(ptr)` to resolve the GPU a device pointer resides on — so that cold-path staging-ring D2D copies and CUDA streams can be directed to the client destination's GPU in multi-GPU deployments. NOTE (as of 2026-07-21): this is an interface keep-up — the receptacle exposes these methods to satisfy the expanded `IGpuServices` trait (currently implemented only by the component's test mock), and the production cold path does NOT yet route transfers by device. The capability is present in the receptacle/mock; per-device routing (`device_of_ptr` → `set_device` before staging-ring copies/streams) is not yet wired into `pipelined_ssd_to_gpu_p2p`.
- **FR-016**: System MUST maintain a persistent per-drive cold-path worker pool (`P2pColdReadPool`) that, at initialization (after the P2P ring is available), pre-allocates one long-lived OS thread plus a pre-connected `ClientChannels` for each (drive, queue-slot) pair (`MAX_QUEUES_PER_DRIVE` slots per drive), eliminating the per-batch `connect_client()` + scoped-thread setup previously required for every cold `batch_lookup`. Cold-read jobs are dispatched to the worker for the target drive over a bounded (depth-1) channel, and the worker executes the P2P pipeline (`pipelined_multi_object_p2p`) and returns per-job results. If pool creation fails at initialization (e.g., `connect_client()` error), the system MUST log a non-fatal diagnostic and fall back, for the remaining lifetime of the component, to the pre-existing inline per-batch path (connect + run the pipeline on the calling thread for each drive/chunk). The pool MUST be signaled to stop and its worker threads released as part of component shutdown, before the P2P ring itself is destroyed.
- **FR-017**: System MUST provide an eviction-event notification channel for observability of memory-tier evictions. `create_eviction_channel(capacity)` registers a bounded `crossbeam_channel::Receiver<EvictionEvent>` for the component (single active subscriber). Every memory-tier eviction performed while serving a lookup or write (`evict_for_space_emit`, covering both the "demote to block device" and "remove" outcomes) attempts to publish an `EvictionEvent { key, reason }` (`EvictionReason::Demoted` or `EvictionReason::Removed`) to the registered channel using a non-blocking `try_send`. Eviction event delivery MUST NOT block, delay, or fail the eviction operation: if the channel is full or no subscriber has been registered, the event MUST be silently dropped and counted, and the running drop count MUST be readable and reset via `eviction_dropped_count()`.
- **FR-018**: System MUST provide a parallel write-through persistence path from the memory tier to SSD using one dedicated writer thread per drive (`ParallelBackgroundWriter`). Each `WriteJob` is routed to the writer owning its target drive (`device_index % num_drives`) and processed asynchronously so that write-through across multiple NVMe devices proceeds concurrently. The writer pool MUST expose in-flight accounting, a `flush()` that blocks until all per-drive queues are drained, and a `shutdown()` that drains remaining jobs and joins all threads (also invoked on drop).
- **FR-019**: System MUST reclaim SSD capacity via a background evictor thread (`BackgroundEvictor`) when configured (`ssd_eviction_threshold > 0.0`). On each `ssd_eviction_interval_secs` cycle it computes aggregate extent-manager utilization; when utilization exceeds `ssd_eviction_threshold` it evicts oldest keys in batches of `ssd_eviction_batch_size`, removing each from the memory tier and dispatch-map and freeing the backing extent, publishing an `EvictionEvent { reason: Removed }` per eviction, and stops once utilization drops below `ssd_eviction_low_watermark`. The evictor MUST honor a shutdown signal promptly (also invoked on drop).
- **FR-020**: System MUST proactively demote LRU entries from DRAM to SSD via a background evictor thread (`MemoryTierEvictor`) when configured (`memory_tier_eviction_threshold > 0.0`, disabled by default at 0.0). On each `memory_tier_eviction_interval_secs` cycle it compares memory-tier utilization against the threshold and, when exceeded, demotes oldest evictable keys (via `try_evict_to_block` + memory-tier `remove`), publishing an `EvictionEvent { reason: Demoted }` per demotion, until utilization drops below `memory_tier_eviction_low_watermark`. Batch aggressiveness scales with pressure (up to 8× `memory_tier_eviction_batch_size` as utilization approaches full); when a sweep demotes nothing (candidates held by in-flight write-through) it backs off and widens the scan window on subsequent dry runs. The evictor MUST honor a shutdown signal promptly (also invoked on drop).
- **FR-021**: System MUST provide an administrative `clear_memory_tier()` operation that flushes the entire memory tier, returning the number of entries cleared. Each entry is demoted to its SSD copy where one exists (`try_evict_to_block`), otherwise force-removed from both the memory tier and the dispatch-map. The operation requires the component to be initialized and its dispatch-map and memory-tier receptacles to be bound.
- **FR-022**: System MUST provide `lookup_async(key, ipc_handle)` returning a `GpuStream` so callers can pipeline hot-path completions on their own schedule. For a memory-tier (hot) hit it issues an asynchronous H2D copy on a dedicated warm CUDA stream (falling back to a synchronous copy when no warm stream is available) and returns the stream without blocking on completion; the caller is responsible for synchronizing the returned stream before consuming the data. Read pins are released and the entry's LRU position is refreshed as part of the operation.
- **FR-023**: System MUST hold dispatch-map read pins for the full lifetime of any asynchronous GPU copy, releasing them only after the copy completes (post `stream_synchronize`), not at submission. A batch guard (`PinnedKeys`) MUST own the adopted read pins and release them exactly once on drop across all exit paths (submission, submit failure, lookup miss, sync failure), because a leaked pin permanently prevents eviction of its entry and is indistinguishable from a live reader. This invariant applies to both the local hot-path async copy and the remote-lookup delivery path.

### Key Entities

- **Staging Ring**: A fixed-size collection of 64 GPU-resident buffer slots shared across cold lookup threads. Allocated once at initialization. Includes 4 pre-allocated CUDA streams for D2D copies.
- **Ring Slot**: An individual buffer within the staging ring. Holds one chunk during transfer. Recyclable after stream sync confirms the D2D copy from that slot has completed.
- **Thread Partition**: A non-overlapping slice of the ring assigned to one cold-path thread. With `MAX_QUEUES_PER_DRIVE=1` and 4 drives, each partition is 16 slots.
- **Dispatch Map**: Routing table indicating whether a lookup key resides in DRAM (hot) or on SSD (cold).
- **P2pColdReadPool**: Persistent pool of per-(drive, queue-slot) worker threads, each owning a pre-connected `ClientChannels`, that execute cold-path P2P pipeline jobs submitted over a bounded channel. Primary cold-path execution model; degrades to an inline per-batch fallback if pool creation fails at init.
- **EvictionEvent / EvictionReason**: Notification emitted on every memory-tier eviction (`Demoted` to block device, or `Removed`), delivered best-effort over a bounded, single-subscriber channel with drop-and-count backpressure semantics.
- **ParallelBackgroundWriter**: Pool of per-drive `BackgroundWriter` threads that route each `WriteJob` to its target drive's writer, giving concurrent memory-tier→SSD write-through across NVMe devices. Supports in-flight accounting, `flush()`, and draining `shutdown()`.
- **BackgroundEvictor**: Periodic SSD capacity-reclamation thread driven by extent-manager utilization watermarks (`ssd_eviction_*` config). Evicts oldest keys down to a low watermark, freeing extents and emitting `Removed` events.
- **MemoryTierEvictor**: Periodic DRAM→SSD demotion thread driven by memory-tier utilization watermarks (`memory_tier_eviction_*` config), with pressure-scaled batch sizing and dry-run backoff, emitting `Demoted` events.
- **PinnedKeys**: A crate-local guard owning a batch of dispatch-map read pins, releasing them together on drop so a pin outlives the completion (not merely submission) of an asynchronous GPU copy, preventing an entry from being demoted while a copy is still reading it.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Cold lookups complete successfully with correct data under single-client and multi-client (4+) workloads.
- **SC-002**: Hot-path throughput shows no measurable regression compared to the standard dispatcher.
- **SC-003**: The system handles 4+ concurrent clients performing cold lookups without data corruption or deadlock.
- **SC-004**: All staging resources are fully released on shutdown with zero leaks.
- **SC-005**: End-to-end throughput is measurable and comparable between the P2P path (full-p2p.yaml) and the DRAM path (full.yaml) using the pipelined benchmark tool.
- **SC-006**: When P2P ring allocation fails (GDRCopy/BAR1 unavailable), initialization logs a clear, non-fatal diagnostic and continues (permitting hot-only testing without P2P hardware). The failure is surfaced fatally on first use: the first cold `batch_lookup` panics with a diagnostic directing the operator to the `full.yaml` profile, while the single-key `lookup()` path silently falls back to the DRAM path. (Consistent with FR-006, FR-007, and User Story 2 AC-1.)

## Assumptions

- The host system has a GPU with sufficient memory to allocate the staging ring.
- Client GPU memory arrives via IPC and cannot be used directly as DMA targets.
- NVMe drives are accessible via userspace drivers.
- The standard dispatcher's interface is stable and will not change during this development.
- Environment initialization (SPDK, GPU runtime) is handled by other components before this component starts.
- The existing pipelined benchmark tool (`certus-api-bench_v2.py`) is available for performance measurement.
