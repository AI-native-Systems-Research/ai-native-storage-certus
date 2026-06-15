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
- **FR-003**: System MUST pre-allocate a fixed ring of 64 GPU staging buffers at initialization via `cudaMalloc` + GDRCopy BAR1 mapping (`gdr_pin_buffer` + `gdr_map`) + `spdk_mem_register`. Each slot is 128 KiB (MDTS). The ring is shared across all cold lookup threads.
- **FR-004**: System MUST partition the staging ring for concurrent thread access using `ThreadPartition` (non-overlapping slot ranges, effective QD capped at 16 per thread to prevent NVMe qpair saturation).
- **FR-005**: System MUST pipeline SSD reads with D2D GPU copies using FIFO completion ordering (no tags). Stream synchronization occurs only when recycling ring slots (not on every completion). No final stream sync — caller is responsible for ensuring completion.
- **FR-006**: System MUST panic on first cold lookup if the P2P ring was not initialized (GDRCopy unavailable, GPU memory insufficient). Initialization logs a diagnostic warning but does not fail, allowing hot-only testing without P2P hardware. No DRAM fallback for cold reads — use the `full.yaml` profile (standard dispatcher) for DRAM-only deployments.
- **FR-007**: The P2P ring is allocated once at initialization and is immutable for the component's lifetime. There is no runtime path selection.
- **FR-008**: System MUST implement the same interface as the standard dispatcher, serving as a drop-in replacement.
- **FR-009**: System MUST promote successfully read cold entries back to DRAM after completing the read.
- **FR-010**: System MUST release all staging resources on shutdown with no leaks.
- **FR-011**: System MUST handle read failures gracefully without corrupting ring state or affecting other in-flight operations.
- **FR-012**: System MUST support end-to-end performance measurement using the existing pipelined benchmark tool.

### Key Entities

- **Staging Ring**: A fixed-size collection of GPU-resident buffer slots shared across cold lookup threads. Allocated once at initialization.
- **Ring Slot**: An individual buffer within the staging ring. Holds one chunk during transfer. Recyclable after the copy to the client destination completes.
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
