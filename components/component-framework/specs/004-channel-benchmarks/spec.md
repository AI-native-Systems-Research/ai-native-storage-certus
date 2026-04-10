# Feature Specification: Channel Backend Benchmarks

**Feature Branch**: `004-channel-benchmarks`
**Created**: 2026-03-31
**Status**: Complete
**Input**: User description: "Implement channels based on crossbeam, kanal, rtrb and tokio lock-free queues. Implement performance benchmarks for different channels available so that their performance can be compared."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Compare Channel Performance (Priority: P1)

As a framework developer, I want to run benchmarks that compare the throughput and latency of all available channel backends (built-in, crossbeam, kanal, rtrb, tokio) so that I can make informed decisions about which channel to use for a given workload.

**Why this priority**: Performance comparison is the core value proposition of this feature. Without benchmarks, developers have no objective basis for selecting a channel backend.

**Independent Test**: Can be fully tested by running the benchmark suite and verifying that each channel backend produces comparable throughput/latency results in a standardized test harness.

**Acceptance Scenarios**:

1. **Given** the benchmark suite is available, **When** a developer runs the benchmarks, **Then** results are produced for every channel backend (built-in SPSC, built-in MPSC, crossbeam-bounded, crossbeam-unbounded, kanal, rtrb, tokio) in a consistent format showing throughput and latency.
2. **Given** the benchmark suite is available, **When** a developer runs the benchmarks with different message sizes (small, medium, large), **Then** results reflect the performance characteristics of each backend at each message size.
3. **Given** the benchmark suite is available, **When** a developer runs SPSC benchmarks, **Then** only backends supporting single-producer single-consumer topology are included in the SPSC group.
4. **Given** the benchmark suite is available, **When** a developer runs MPSC benchmarks, **Then** only backends supporting multi-producer single-consumer topology are included in the MPSC group.

---

### User Story 2 - Use Third-Party Channel Backends as Components (Priority: P2)

As a framework developer, I want to use crossbeam, kanal, rtrb, and tokio-based channels as drop-in replacements for the built-in channels within the component framework, so that I can select the best-performing backend for my use case without changing application code.

**Why this priority**: The channel backends must be usable as components (implementing the same interfaces) before they can be meaningfully benchmarked in a component-framework context. This enables the benchmarks in US1.

**Independent Test**: Can be fully tested by creating each third-party channel backend, querying it for ISender/IReceiver interfaces, and sending/receiving messages through those interfaces.

**Acceptance Scenarios**:

1. **Given** a crossbeam-bounded channel component, **When** a developer queries it for ISender and IReceiver, **Then** the interfaces are returned and messages can be sent and received.
1. **Given** a crossbeam-unbounded channel component, **When** a developer queries it for ISender and IReceiver, **Then** the interfaces are returned and messages can be sent and received.
2. **Given** a kanal-based channel component, **When** a developer queries it for ISender and IReceiver, **Then** the interfaces are returned and messages can be sent and received.
3. **Given** an rtrb-based channel component (SPSC only), **When** a developer queries it for ISender and IReceiver, **Then** the interfaces are returned and messages can be sent and received, with SPSC binding constraints enforced.
4. **Given** a tokio-based channel component, **When** a developer queries it for ISender and IReceiver, **Then** the interfaces are returned and messages can be sent and received.
5. **Given** any third-party channel component, **When** a developer inspects it via introspection (provided_interfaces, version), **Then** the component reports ISender and IReceiver as provided interfaces.

---

### User Story 3 - Benchmark Different Topologies (Priority: P3)

As a framework developer, I want to benchmark channels across different producer/consumer topologies (1:1 SPSC, N:1 MPSC) and varying queue depths, so that I can understand how each backend behaves under different concurrency patterns.

**Why this priority**: Real-world workloads vary in topology and contention. Understanding how backends perform under different configurations is essential for production use.

**Independent Test**: Can be fully tested by running topology-specific benchmark groups and verifying that results include multiple producer counts and queue capacities.

**Acceptance Scenarios**:

1. **Given** the benchmark suite, **When** a developer runs SPSC benchmarks, **Then** results include throughput for 1-producer/1-consumer with at least two different queue capacities.
2. **Given** the benchmark suite, **When** a developer runs MPSC benchmarks, **Then** results include throughput for at least 2, 4, and 8 producers with 1 consumer.
3. **Given** the benchmark suite, **When** a developer runs benchmarks with varying queue depths (e.g., 64, 1024, 16384), **Then** results show the impact of queue capacity on throughput and latency.

---

### Edge Cases

- What happens when a channel backend does not support a requested topology (e.g., rtrb does not support MPSC)? The benchmark suite MUST skip unsupported combinations gracefully.
- How does system handle a backend that panics or returns errors during benchmarking? The benchmark MUST report the failure without crashing the entire suite.
- What happens when message sizes exceed the channel's internal buffer? The benchmark MUST use blocking send/receive semantics consistent with the built-in channels.
- How does performance degrade when the queue is saturated (producer faster than consumer)? Benchmarks MUST include a back-pressure scenario.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The framework MUST provide channel components backed by crossbeam unbounded and bounded channels supporting both SPSC and MPSC topologies.
- **FR-002**: The framework MUST provide channel components backed by kanal bounded channels supporting both SPSC and MPSC topologies.
- **FR-003**: The framework MUST provide channel components backed by rtrb bounded ring buffers supporting SPSC topology only.
- **FR-004**: The framework MUST provide channel components backed by tokio bounded MPSC channels supporting MPSC topology. Tokio SPSC (oneshot) is out of scope.
- **FR-005**: Each third-party channel component MUST implement the same component model interface (IUnknown) as the built-in channels, providing ISender and IReceiver as queryable interfaces.
- **FR-006**: Each third-party channel component MUST enforce the same binding constraints as its topology implies (SPSC backends reject a second sender or receiver; MPSC backends reject a second receiver but allow multiple senders).
- **FR-007**: Each third-party channel component MUST support introspection (provided_interfaces, receptacles, version) consistent with the existing component model.
- **FR-008**: The framework MUST provide a benchmark suite that measures throughput (messages per second) for each channel backend under standardized conditions.
- **FR-009**: The benchmark suite MUST measure latency (time per message round-trip or send-to-receive) for each channel backend.
- **FR-010**: The benchmark suite MUST include SPSC benchmark groups comparing all SPSC-capable backends side by side.
- **FR-011**: The benchmark suite MUST include MPSC benchmark groups comparing all MPSC-capable backends side by side with at least 2, 4, and 8 producer threads.
- **FR-012**: The benchmark suite MUST test at least two different message sizes (small fixed-size and larger variable-size) to reveal serialization and memory-copy costs.
- **FR-013**: The benchmark suite MUST test at least two different queue capacities to reveal back-pressure and cache behavior.
- **FR-014**: All third-party channel components MUST have unit tests verifying send/receive correctness, binding enforcement, and channel closure semantics.
- **FR-015**: All third-party channel components MUST have doc tests on public types.
- **FR-016**: The benchmark suite MUST produce results that are directly comparable across backends (same message count, same thread counts, same measurement methodology).

### Key Entities

- **Channel Backend**: A specific queue implementation (built-in, crossbeam-bounded, crossbeam-unbounded, kanal, rtrb, tokio) wrapped as a component providing ISender/IReceiver.
- **Benchmark Group**: A collection of benchmarks sharing the same topology (SPSC or MPSC) and configuration, comparing all compatible backends.
- **Benchmark Configuration**: Parameters defining a benchmark run — message count, message size, queue capacity, producer count.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All channel backends pass a correctness test suite verifying zero-loss sequential message delivery of at least 100,000 messages.
- **SC-002**: The benchmark suite produces throughput and latency results for all channel backends in under 5 minutes total execution time.
- **SC-003**: Each benchmark result includes the backend name, topology, message count, and measured metric so that results are self-describing and comparable.
- **SC-004**: Developers can select any third-party channel backend as a drop-in replacement for built-in channels without changing application-level send/receive code.
- **SC-005**: All third-party channel components pass the same binding enforcement tests as built-in channels (SPSC rejects second sender/receiver; MPSC rejects second receiver).
- **SC-006**: The benchmark suite runs without manual configuration — a single command produces all results.
- **SC-007**: All public types and functions have doc tests, and all tests pass with zero failures.

## Assumptions

- The third-party channel crates (crossbeam-channel, kanal, rtrb, tokio) are added as dependencies. This is the first feature requiring external crate dependencies beyond proc-macro crates.
- tokio channels are used in a synchronous (blocking) context for benchmark fairness; async runtimes are not required.
- rtrb is SPSC-only by design; it will not appear in MPSC benchmark groups.
- Benchmarks use the existing Criterion setup (already present in the project) for statistically rigorous measurement.
- Message types used in benchmarks are simple (e.g., u64 for small, Vec<u8> for larger) to isolate channel overhead from payload processing.
- The existing ISender/IReceiver traits and IUnknown pattern are reused without modification; new backends adapt to the existing interface.
- Crossbeam provides both bounded and unbounded channel components as distinct backends; both are included in benchmarks. Note that unbounded channels have different back-pressure characteristics and benchmark results should be interpreted accordingly.

## Clarifications

### Session 2026-03-31

- Q: Should crossbeam provide both bounded and unbounded channel components, or just bounded? → A: Both bounded and unbounded (two separate components).
