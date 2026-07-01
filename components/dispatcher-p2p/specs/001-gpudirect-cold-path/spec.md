# Feature Specification: GPUDirect Storage Cold Path

**Feature Branch**: `p2p_component`

**Created**: 2026-06-11

**Status**: Draft

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
- **FR-008**: System MUST implement the same interface as the standard dispatcher, serving as a drop-in replacement.
- **FR-009**: System MUST asynchronously promote cold entries to DRAM via a throttled background worker (`DramBackfillWorker`) after serving the client via P2P. The worker re-reads data from SSD into the memory-tier slot, then registers the key as MemoryTier in the dispatch-map. During the backfill window, repeat lookups of the same key use the P2P cold path (correct data, no stale DRAM). The backfill delay is controlled by `backfill_delay_ms` in `DispatcherConfig`.
- **FR-010**: System MUST release all staging resources on shutdown with no leaks.
- **FR-011**: System MUST handle read failures gracefully without corrupting ring state or affecting other in-flight operations.
- **FR-012**: Performance measurement is handled by external benchmarking tools (e.g., `certus-api-bench_v2.py`) rather than built-in hooks, to avoid instrumentation overhead in the production path.
- **FR-013**: System MUST implement `promote_to_memory_tier(keys)` to asynchronously read cold entries from NVMe into the memory-tier without GPU involvement, enabling future lookups to take the hot DRAM→GPU path. This uses the `pipelined_ssd_to_dram_only` pipeline function (one thread per drive, no P2P ring involvement).
- **FR-014**: System MUST support configurable DRAM backfill throttling via `backfill_delay_ms` in `DispatcherConfig`. Default: 10ms. When set to 0, no background DRAM backfill occurs and cold-promoted keys remain as BlockDevice indefinitely (repeat lookups always use P2P). When > 0, the `DramBackfillWorker` sleeps for that duration between jobs to avoid contending with active P2P cold reads for NVMe bandwidth.

### Key Entities

- **Staging Ring**: A fixed-size collection of 64 GPU-resident buffer slots shared across cold lookup threads. Allocated once at initialization. Includes 4 pre-allocated CUDA streams for D2D copies.
- **Ring Slot**: An individual buffer within the staging ring. Holds one chunk during transfer. Recyclable after stream sync confirms the D2D copy from that slot has completed.
- **Thread Partition**: A non-overlapping slice of the ring assigned to one cold-path thread. With `MAX_QUEUES_PER_DRIVE=1` and 4 drives, each partition is 16 slots.
- **Dispatch Map**: Routing table indicating whether a lookup key resides in DRAM (hot) or on SSD (cold).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Cold lookups complete successfully with correct data under single-client and multi-client (4+) workloads.
- **SC-002**: Hot-path throughput shows no measurable regression compared to the standard dispatcher.
- **SC-003**: The system handles 4+ concurrent clients performing cold lookups without data corruption or deadlock.
- **SC-004**: All staging resources are fully released on shutdown with zero leaks.
- **SC-005**: End-to-end throughput is measurable and comparable between the P2P path (full-p2p.yaml) and the DRAM path (full.yaml) using the pipelined benchmark tool.
- **SC-006**: Initialization panics with a clear diagnostic when P2P ring allocation fails (GDRCopy/BAR1 unavailable).

## Assumptions

- The host system has a GPU with sufficient memory to allocate the staging ring.
- Client GPU memory arrives via IPC and cannot be used directly as DMA targets.
- NVMe drives are accessible via userspace drivers.
- The standard dispatcher's interface is stable and will not change during this development.
- Environment initialization (SPDK, GPU runtime) is handled by other components before this component starts.
- The existing pipelined benchmark tool (`certus-api-bench_v2.py`) is available for performance measurement.
