# Feature Specification: Extent Manager Benchmark Application

**Feature**: `002-extent-benchmark`  
**Created**: 2026-04-16  
**Status**: Draft  
**Input**: User description: "Create a benchmarking application, apps/extent-benchmark, for this component that measures latencies and throughput of extent allocation, lookup and remove/deletion. The application should allow multiple client threads through a --threads option. Add a README.md summarizing the app and giving instructions on how to run with real SPDK NVMe block device."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Single-Threaded Extent Benchmark (Priority: P1)

A developer or performance engineer runs the benchmark application against a real NVMe SSD to establish baseline latency and throughput numbers for the three core extent manager operations: create, lookup, and remove. The tool runs each benchmark phase sequentially and reports per-operation latency statistics (min, max, mean, p50, p99) and aggregate throughput (ops/sec).

**Why this priority**: This is the foundational use case — all other functionality builds on the ability to measure individual operations. A single-threaded run isolates extent manager performance from contention effects.

**Independent Test**: Can be fully tested by running `extent-benchmark --device <pci-addr> --ns-id 1 --threads 1` against a real NVMe device and verifying latency/throughput output is printed for create, lookup, and remove phases.

**Acceptance Scenarios**:

1. **Given** an NVMe device is available and SPDK environment is configured, **When** the user runs `extent-benchmark --device 0000:03:00.0 --ns-id 1 --threads 1`, **Then** the tool initializes the extent manager, runs create/lookup/remove benchmarks, and prints latency histograms and throughput for each phase.
2. **Given** the tool is run with default parameters, **When** the benchmark completes, **Then** each phase reports: operation count, total elapsed time, ops/sec, and latency percentiles (min, p50, p99, max).
3. **Given** the user specifies `--count 50000`, **When** the benchmark runs, **Then** exactly 50,000 operations are performed per phase (create 50K, lookup 50K, remove 50K).

---

### User Story 2 - Multi-Threaded Scalability Benchmark (Priority: P2)

A performance engineer runs the benchmark with multiple client threads (`--threads N`) to measure how extent manager throughput and latency scale under concurrent access. Each thread operates on its own key range to avoid contention at the key level while exercising the shared write lock in the extent manager.

**Why this priority**: Multi-threaded performance is critical for production workloads where many clients access the extent manager concurrently. Understanding lock contention and scalability informs capacity planning.

**Independent Test**: Can be tested by running `extent-benchmark --device <pci-addr> --ns-id 1 --threads 4` and verifying per-thread and aggregate statistics are reported.

**Acceptance Scenarios**:

1. **Given** the user specifies `--threads 4`, **When** the benchmark runs, **Then** 4 worker threads each perform their share of operations concurrently, and per-thread and aggregate results are reported.
2. **Given** multi-threaded mode, **When** the benchmark completes, **Then** the report includes aggregate throughput (total ops/sec across all threads) and per-thread latency distributions.

---

### User Story 3 - Configurable Benchmark Parameters (Priority: P3)

A user customizes the benchmark via command-line options: operation count, size class, slab size, and total device space. This allows testing different workload profiles and device configurations.

**Why this priority**: Configurability enables targeted performance investigations (e.g., measuring how slab size affects allocation throughput, or how different size classes perform).

**Independent Test**: Can be tested by running with various flag combinations and verifying the benchmark respects each parameter.

**Acceptance Scenarios**:

1. **Given** the user specifies `--size-class 262144`, **When** the benchmark runs, **Then** all extents are created with the 256 KiB size class.
2. **Given** the user specifies `--slab-size 1073741824`, **When** the benchmark runs, **Then** the extent manager is initialized with 1 GiB slabs.
3. **Given** the user specifies `--total-size 107374182400`, **When** the benchmark runs, **Then** the extent manager is initialized with 100 GiB of managed space.

---

### Edge Cases

- What happens when the device runs out of space during the create phase? The tool reports how many creates succeeded and the error, then continues to the lookup and remove phases with the extents that were created.
- What happens when the specified PCI address is invalid or no device is found? The tool prints a clear error message and exits with a non-zero status code.
- What happens when `--threads` exceeds the number of available CPU cores? The tool warns but proceeds (OS scheduling handles oversubscription).
- What happens when `--count` is not evenly divisible by `--threads`? The tool distributes operations as evenly as possible (some threads may do one extra operation).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The application MUST be a standalone binary crate at `apps/extent-benchmark/` within the workspace.
- **FR-002**: The application MUST accept `--device <PCI_ADDRESS>` to specify the NVMe device.
- **FR-003**: The application MUST accept `--ns-id <NAMESPACE_ID>` to specify the NVMe namespace (default: 1).
- **FR-004**: The application MUST accept `--threads <N>` to specify the number of worker threads (default: 1).
- **FR-005**: The application MUST accept `--count <N>` to specify operations per phase (default: 10,000).
- **FR-006**: The application MUST accept `--size-class <BYTES>` to specify the extent size class (default: 131072 / 128 KiB).
- **FR-007**: The application MUST accept `--slab-size <BYTES>` to specify slab size for initialization (default: 1073741824 / 1 GiB).
- **FR-008**: The application MUST accept `--total-size <BYTES>` to specify total managed space (default: auto-detect from device capacity).
- **FR-009**: The application MUST wire up the component stack: Logger, SPDKEnv, BlockDeviceSpdkNvme, and ExtentManagerComponentV1. Logger is bound to all components that declare an ILogger receptacle.
- **FR-010**: The application MUST run three benchmark phases in order: create, lookup, remove.
- **FR-011**: Each worker thread MUST operate on a disjoint key range to avoid duplicate-key errors.
- **FR-012**: The application MUST report per-phase statistics: total ops, elapsed time, ops/sec, and latency percentiles (min, p50, p99, max).
- **FR-013**: In multi-threaded mode, the application MUST report both per-thread and aggregate statistics.
- **FR-014**: The application MUST include a README.md with build instructions, usage examples, and prerequisites (hugepages, VFIO, SPDK setup).
- **FR-015**: If a phase encounters errors partway through, the application MUST report partial results and continue to subsequent phases.

### Key Entities

- **BenchmarkConfig**: Captures all CLI parameters (device, namespace, threads, count, size class, slab size, total size).
- **PhaseResult**: Statistics for one benchmark phase (operation type, count, elapsed, latency samples).
- **WorkerResult**: Per-thread results including latency samples for aggregation.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A user can run the benchmark and receive results within 5 minutes for the default 10,000-operation workload.
- **SC-002**: The benchmark produces repeatable results — consecutive runs on the same device with the same parameters yield throughput numbers within 10% of each other.
- **SC-003**: Multi-threaded mode correctly distributes work — N threads each process their share of operations concurrently. For I/O-bound operations (create, remove), aggregate throughput improves with additional threads. For in-memory operations (lookup), per-operation latency remains sub-microsecond regardless of thread count.
- **SC-004**: The README enables a new team member to build and run the benchmark on a properly configured SPDK system without additional assistance.
- **SC-005**: Latency percentile reporting is accurate to microsecond resolution.

## Assumptions

- The target system has SPDK prerequisites configured: hugepages allocated, VFIO/UIO driver bound to the NVMe device, and the SPDK environment libraries available.
- The NVMe device has at least one namespace with sufficient capacity for the benchmark workload.
- The application runs on Linux x86_64 only (same platform constraint as the rest of Certus).
- The existing `iops-benchmark` app at `apps/iops-benchmark/` serves as the architectural reference for component wiring, SPDK initialization, and worker thread patterns.
- NUMA-aware thread pinning follows the same pattern as `iops-benchmark` (pin to controller's NUMA node).
- The `clap` crate is used for CLI argument parsing (consistent with existing apps).
