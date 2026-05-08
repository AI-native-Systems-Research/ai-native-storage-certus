# Feature Specification: IOPS Benchmark Example Application

**Feature Branch**: `002-iops-benchmark`
**Created**: 2026-04-14
**Status**: Draft
**Input**: User description: "Write an example application for this component that measures IOPS throughput for read and write operations. The example should take operation type (read,write,rw), block size, IO queue depth, number of client threads and test duration in seconds. Default values for these should be available."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Run a Basic IOPS Benchmark (Priority: P1)

A performance engineer launches the benchmark application against an
NVMe device to measure raw IOPS throughput. They run it with default
parameters to get a quick baseline, then adjust parameters to explore
different workload profiles. The application prints a summary of IOPS,
throughput, and latency statistics when the test completes.

**Why this priority**: The core purpose of the application is to measure
IOPS. Without this, nothing else is useful.

**Independent Test**: Can be verified by running the benchmark with
defaults and confirming it prints IOPS, throughput (MB/s), and latency
statistics after the configured duration elapses.

**Acceptance Scenarios**:

1. **Given** the benchmark application is launched with no arguments,
   **When** the test duration elapses, **Then** IOPS and throughput
   results are printed to stdout in a human-readable format.
2. **Given** the benchmark application is launched with `--op read`,
   **When** the test completes, **Then** only read operations are
   performed and reported.
3. **Given** the benchmark application is launched with `--op write`,
   **When** the test completes, **Then** only write operations are
   performed and reported.
4. **Given** the benchmark application is launched with `--op rw`,
   **When** the test completes, **Then** both read and write operations
   are performed and their IOPS are reported separately.

---

### User Story 2 - Configure Workload Parameters (Priority: P1)

A performance engineer customizes the benchmark to match a target
workload: selecting block size (e.g. 4KB, 64KB, 1MB), IO queue depth,
number of client threads, and test duration to stress-test specific
aspects of the NVMe device under controlled conditions.

**Why this priority**: Configurability is essential for meaningful
benchmarks. Different workloads reveal different performance
characteristics.

**Independent Test**: Can be verified by running the benchmark with
non-default parameters (e.g. `--block-size 65536 --queue-depth 32
--threads 4 --duration 30`) and confirming the parameters are reflected
in the output header and the benchmark runs accordingly.

**Acceptance Scenarios**:

1. **Given** the application is launched with `--block-size 65536`,
   **When** the test runs, **Then** all IO operations use 64KB blocks.
2. **Given** the application is launched with `--queue-depth 64`,
   **When** the test runs, **Then** the IO queue depth per thread is 64.
3. **Given** the application is launched with `--threads 4`, **When**
   the test runs, **Then** four client threads submit IO concurrently.
4. **Given** the application is launched with `--duration 10`, **When**
   10 seconds elapse, **Then** the benchmark stops and prints results.

---

### User Story 3 - View Live Progress During Benchmark (Priority: P2)

A performance engineer running a long benchmark (e.g. 60 seconds) wants
periodic feedback showing that the benchmark is progressing and
approximately what throughput is being achieved so far.

**Why this priority**: For longer runs, users need assurance the
benchmark is active and can observe convergence of results.

**Independent Test**: Can be verified by running a 10-second benchmark
and confirming periodic progress lines appear on stderr at one-second
intervals.

**Acceptance Scenarios**:

1. **Given** the benchmark is running, **When** each one-second interval
   elapses, **Then** a progress line showing elapsed time and
   instantaneous IOPS is printed to stderr.
2. **Given** the benchmark is running with `--quiet`, **When** the test
   runs, **Then** no progress output appears; only the final summary is
   printed.

---

### User Story 4 - Validate Configuration Before Running (Priority: P3)

A user provides invalid parameters (e.g. block size that is not a
multiple of the device sector size, zero threads, zero duration). The
application validates all parameters at startup and reports clear error
messages before performing any IO.

**Why this priority**: Early validation prevents wasted time and
confusing error messages from the driver layer.

**Independent Test**: Can be verified by launching with intentionally
invalid parameters and confirming a clear error message is printed and
the application exits with a non-zero status code.

**Acceptance Scenarios**:

1. **Given** a block size that is not a multiple of the device sector
   size, **When** the application starts, **Then** it prints an error
   message and exits without performing IO.
2. **Given** `--threads 0` or `--duration 0`, **When** the application
   starts, **Then** it prints an error message and exits without
   performing IO.
3. **Given** a queue depth that exceeds the device's maximum, **When**
   the application starts, **Then** it prints a warning and clamps the
   queue depth to the device maximum.

---

### Edge Cases

- What happens when the NVMe device has no active namespaces? The application should report an error and exit.
- What happens when the device runs out of DMA memory for the requested queue depth and block size? The application should report the allocation failure and exit cleanly.
- What happens when a thread encounters an IO error mid-benchmark? The error is counted and reported in the final summary; the thread continues.
- How does `rw` mode distribute reads and writes? 50/50 ratio, randomly interleaved per operation.

## Clarifications

### Session 2026-04-15

- Q: How does the user select which NVMe controller when multiple devices are present? → A: Add a `--pci-addr` flag accepting a PCI BDF address (e.g., `0000:03:00.0`).
- Q: Should IO access pattern be configurable (random vs sequential)? → A: Yes, add a `--pattern` flag with values `random` (default) and `sequential`.
- Q: Should the application support machine-readable output (e.g., JSON)? → A: No, human-readable text only; machine-readable format is out of scope.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The application MUST accept a command-line flag `--op` with values `read`, `write`, or `rw` to select the operation type. Default: `read`.
- **FR-002**: The application MUST accept a command-line flag `--block-size` specifying the IO block size in bytes. Default: 4096 (4KB).
- **FR-003**: The application MUST accept a command-line flag `--queue-depth` specifying the number of outstanding IO operations per thread. Default: 32.
- **FR-004**: The application MUST accept a command-line flag `--threads` specifying the number of concurrent client threads. Default: 1.
- **FR-005**: The application MUST accept a command-line flag `--duration` specifying the test duration in seconds. Default: 10.
- **FR-006**: The application MUST accept a command-line flag `--ns-id` specifying which NVMe namespace to target. Default: 1.
- **FR-006a**: The application MUST accept a command-line flag `--pci-addr` specifying the NVMe controller by PCI BDF address (e.g., `0000:03:00.0`). If not specified, the application MUST use the first available NVMe controller.
- **FR-006b**: The application MUST accept a command-line flag `--pattern` with values `random` (default) and `sequential` to select the IO access pattern.
- **FR-007**: The application MUST validate all parameters at startup and exit with a clear error message if any parameter is invalid. Specifically: block size must be a positive multiple of the device sector size, threads must be >= 1, duration must be >= 1, queue depth must be >= 1.
- **FR-008**: If the requested queue depth exceeds the device's maximum, the application MUST clamp it to the device maximum and print a warning.
- **FR-009**: Each client thread MUST connect to the block device component via the IBlockDevice interface, obtaining its own channel pair.
- **FR-010**: Each client thread MUST submit IO operations using asynchronous commands with the configured queue depth, keeping the pipeline full for the duration of the test.
- **FR-011**: In `rw` mode, each operation MUST be randomly selected as either a read or a write with equal probability (50/50).
- **FR-012**: The application MUST print a configuration summary at startup showing all active parameters (operation type, block size, queue depth, threads, duration, namespace id, PCI address, access pattern).
- **FR-013**: The application MUST print per-second progress to stderr showing elapsed time and instantaneous IOPS, unless `--quiet` is specified.
- **FR-014**: After the test duration elapses, the application MUST signal all threads to stop and collect their results.
- **FR-015**: The application MUST print a final results summary to stdout containing: total IOPS, throughput in MB/s, and latency statistics (min, mean, p50, p99, max) in microseconds.
- **FR-016**: In `rw` mode, the final summary MUST report read and write IOPS separately in addition to the combined total.
- **FR-017**: When `--pattern random` (the default), each client thread MUST target uniformly random LBA offsets within the namespace capacity. When `--pattern sequential`, each client thread MUST issue IO to contiguous LBA ranges, with each thread targeting a distinct non-overlapping region of the namespace.
- **FR-018**: The application MUST count and report any IO errors encountered during the benchmark without aborting the test.
- **FR-019**: The application MUST exit with status code 0 on success and non-zero on validation failure or fatal errors.
- **FR-020**: The application MUST accept a `--quiet` flag that suppresses per-second progress output.
- **FR-021**: The application MUST accept a `--help` flag that prints usage information and exits.
- **FR-022**: The application MUST accept a command-line flag `--io-mode` with values `sync` and `async` to select the IO submission mode. Default: `async`. In `sync` mode, each IO operation blocks until completion before the next is submitted (effective queue depth 1 per thread regardless of `--queue-depth`). In `async` mode, operations are submitted asynchronously and the pipeline is kept full to `--queue-depth`. The active IO mode MUST be included in the configuration summary output (FR-012).

### Key Entities

- **Benchmark Configuration**: The set of parameters controlling the benchmark run (operation type, block size, queue depth, threads, duration, namespace id, PCI address, access pattern, quiet mode).
- **Thread Result**: Per-thread statistics collected during the benchmark (operation count by type, latency samples, error count).
- **Final Report**: Aggregated results across all threads (total IOPS, throughput, latency percentiles, error count).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The application can be launched with default parameters and produces a valid IOPS result within the configured duration plus a 2-second startup/teardown margin.
- **SC-002**: When run with `--threads N`, exactly N client connections are established with the block device component.
- **SC-003**: The measured IOPS for sequential single-thread 4KB random reads is within 10% of the value reported by a reference tool (e.g., fio) on the same device under equivalent conditions.
- **SC-004**: The application validates all parameters and rejects invalid configurations before performing any IO.
- **SC-005**: Per-second progress output reflects the actual instantaneous throughput (not stale or zero values) within 20% accuracy.
- **SC-006**: The final latency statistics (min, mean, p50, p99, max) are computed correctly from the actual IO completion times, verified by cross-checking with telemetry output from the component.
- **SC-007**: The application handles IO errors gracefully, counting them in the report without crashing or hanging.

## Assumptions

- The NVMe device is already bound to SPDK (via VFIO/UIO) and the SPDK environment is initialized before the benchmark application runs.
- The block device component (spec 001) is fully functional with async IO support.
- The target namespace exists and has sufficient capacity for the benchmark (at least 1GB recommended).
- The application runs on the same host as the NVMe device (local, in-process usage).
- Latency percentile computation (p50, p99) uses a histogram or sorted-sample approach; exact algorithm is an implementation detail.
- The random LBA selection uses a uniform distribution across the namespace; cryptographic randomness is not required.
- The benchmark application is a binary crate located in the workspace (e.g., `apps/iops-benchmark/` or `examples/`).
- Machine-readable output formats (JSON, CSV) are out of scope; the application produces human-readable text only.
- Write operations use a fixed repeating byte pattern (e.g., `0xAA`); the data content is not significant for throughput measurement.
