# Feature Specification: NUMA-Aware Actor Thread Pinning and Memory Allocation

**Feature Branch**: `005-numa-aware-actors`
**Created**: 2026-03-31
**Status**: Complete
**Input**: User description: "The framework should be NUMA-aware and allow actor threads to be bound to one or more CPUs. Any performance tests should analyze threads bound to the same NUMA zone and also to different NUMA zones. You can assume that all systems have at least 2 NUMA zones. Include an example of using NUMA pinning."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Pin Actor Thread to Specific CPUs (Priority: P1)

A framework user creates an actor and specifies which CPU core(s) its dedicated thread should be pinned to. After activation, the actor's thread runs exclusively on the specified core(s). This gives the user explicit control over thread placement for latency-sensitive workloads, cache locality, and NUMA-local memory access.

**Why this priority**: Thread pinning is the foundational capability. Without it, NUMA-aware benchmarks and examples cannot exist. Pinning a single actor to specific CPUs is the minimal useful unit of work.

**Independent Test**: Create an actor with a CPU affinity set, activate it, send a message, and verify the actor's thread is running on the specified CPU(s).

**Acceptance Scenarios**:

1. **Given** an actor configured with affinity for CPU 0, **When** the actor is activated and processes a message, **Then** the thread reports CPU 0 as its current processor.
2. **Given** an actor configured with affinity for CPUs {2, 3}, **When** the actor is activated, **Then** the thread's affinity mask includes exactly CPUs 2 and 3.
3. **Given** an actor with no affinity configured (default), **When** the actor is activated, **Then** the thread runs with the system default scheduling (no pinning applied) — full backward compatibility.
4. **Given** an actor configured with an invalid CPU ID (e.g., CPU 9999 on a 16-core system), **When** the actor is activated, **Then** a clear error is returned indicating the CPU ID is out of range.

---

### User Story 2 - Query NUMA Topology at Runtime (Priority: P1)

A framework user queries the system's NUMA topology to discover how many NUMA nodes exist, which CPUs belong to each node, and how nodes relate to each other. This information drives intelligent placement decisions — users pick CPUs from the same node for latency-sensitive pairs or from different nodes to test cross-node penalties.

**Why this priority**: Without topology discovery, users must hard-code CPU IDs, making their code non-portable. Topology queries are essential for writing NUMA-aware code that works across different machines.

**Independent Test**: Query the NUMA topology on the current machine and verify the result reports at least 1 node, each containing at least 1 CPU, with all system CPUs accounted for.

**Acceptance Scenarios**:

1. **Given** a system with 2 or more NUMA nodes, **When** the user queries the NUMA topology, **Then** the result contains at least 2 nodes with their associated CPU lists.
2. **Given** the topology result, **When** the user iterates over all nodes, **Then** every online CPU appears in exactly one node.
3. **Given** the topology result, **When** the user asks for the CPUs of a specific node, **Then** a non-empty list of CPU IDs is returned.

---

### User Story 3 - NUMA-Aware Performance Benchmarks (Priority: P2)

A performance engineer runs benchmarks that measure actor-to-actor message passing latency under two NUMA configurations: (a) both actors pinned to CPUs on the same NUMA node, and (b) actors pinned to CPUs on different NUMA nodes. The benchmark results quantify the cross-node penalty and help users make informed placement decisions.

**Why this priority**: Benchmarks validate that pinning works correctly and provide actionable data. They depend on both thread pinning (US1) and topology discovery (US2) being functional first.

**Independent Test**: Run the NUMA benchmark suite and verify it produces latency measurements for both same-node and cross-node configurations, with cross-node latency measurably higher than same-node latency.

**Acceptance Scenarios**:

1. **Given** a system with at least 2 NUMA nodes, **When** the benchmark runs the same-node configuration, **Then** both actors are pinned to CPUs on the same NUMA node and latency results are recorded.
2. **Given** a system with at least 2 NUMA nodes, **When** the benchmark runs the cross-node configuration, **Then** the two actors are pinned to CPUs on different NUMA nodes and latency results are recorded.
3. **Given** both benchmark configurations have completed, **When** the results are compared, **Then** cross-node latency is measurably higher than same-node latency (demonstrates NUMA effect).
4. **Given** a system with at least 2 NUMA nodes, **When** the throughput benchmark runs, **Then** messages-per-second is recorded for both same-node and cross-node configurations.

---

### User Story 4 - NUMA Pinning Example (Priority: P3)

A developer learning the framework runs a self-contained example that demonstrates NUMA-aware actor pinning. The example discovers the system topology, creates two actors pinned to specific NUMA nodes, exchanges messages between them, and prints the observed CPU placement and round-trip latency.

**Why this priority**: The example is documentation and a learning aid. It depends on all other capabilities being functional.

**Independent Test**: The example compiles, runs without errors on a multi-NUMA system, and prints topology information, CPU placements, and latency measurements.

**Acceptance Scenarios**:

1. **Given** the NUMA pinning example, **When** run on a system with at least 2 NUMA nodes, **Then** it prints the detected NUMA topology, pins actors to specific nodes, exchanges messages, and reports per-node latency.
2. **Given** the NUMA pinning example, **When** run on a single-NUMA-node system, **Then** it prints a warning that cross-node comparison is not possible and runs both actors on the available node.

---

### Edge Cases

- What happens when the specified CPU ID does not exist on the system? The framework MUST return a clear error before spawning the thread.
- What happens when the user specifies an empty CPU set? The framework MUST return an error indicating at least one CPU must be specified.
- What happens when the OS denies the affinity-set request (e.g., insufficient privileges)? The framework MUST propagate the OS error as a descriptive error to the caller.
- What happens when a CPU is offline? The framework MUST detect offline CPUs and exclude them from topology results; pinning to an offline CPU MUST return an error.
- What happens when topology information is unavailable (e.g., a VM that hides NUMA details)? The framework MUST fall back to reporting a single NUMA node containing all CPUs.
- What happens when an actor is deactivated and reactivated with a different CPU affinity? The new affinity MUST apply to the newly spawned thread.

## Requirements *(mandatory)*

### Functional Requirements

**Thread Pinning**:

- **FR-001**: The framework MUST allow specifying a CPU affinity set (one or more CPU IDs) when creating an actor, and MUST allow changing the affinity while the actor is idle (between activation cycles).
- **FR-002**: When an actor is activated, its dedicated thread MUST be pinned to the specified CPU affinity set before the message loop begins.
- **FR-003**: If no CPU affinity is specified, the actor MUST behave identically to the current implementation (no pinning, full backward compatibility).
- **FR-004**: The framework MUST validate CPU IDs against the system's available CPUs and return an error for invalid IDs before spawning the thread.
- **FR-005**: The framework MUST return a descriptive error if the OS rejects the affinity-set operation.
- **FR-006**: An empty CPU affinity set MUST be rejected with a clear error.

**NUMA-Local Memory Allocation**:

- **FR-015**: The framework MUST provide a NUMA-local allocator that allocates memory on a specified NUMA node.
- **FR-016**: Channel buffers rely on Linux first-touch memory policy for
  NUMA locality. The `new_numa()` constructors accept a node parameter for
  API consistency and documentation but delegate to the standard constructor.
  When the constructing thread is pinned to CPUs on a specific NUMA node,
  the OS allocates channel buffer pages on that node when first accessed.
  Explicit `mbind()` is not used for channel buffers because it interferes
  with the Rust allocator's internal bookkeeping.
- **FR-017**: Actor handler state achieves NUMA-local placement through
  first-touch policy when the handler is constructed on a thread pinned to
  the target NUMA node's CPUs. The framework does not provide explicit
  NUMA-local allocation for handler state; users requiring strict placement
  should construct the handler on a pinned thread before passing it to the
  actor constructor.
- **FR-018**: If no NUMA affinity is specified, memory allocation MUST use the system default policy (no change from current behavior).
- **FR-019**: The NUMA-local allocator MUST fall back to system default allocation if NUMA-aware allocation is unavailable or fails.

**NUMA Topology Discovery**:

- **FR-007**: The framework MUST provide a way to query the system's NUMA topology at runtime, returning the number of NUMA nodes and the CPUs belonging to each node.
- **FR-008**: The topology query MUST account for all online CPUs, with each CPU appearing in exactly one NUMA node.
- **FR-009**: On systems where NUMA information is unavailable, the framework MUST fall back to reporting a single node containing all online CPUs.

**Benchmarks**:

- **FR-010**: The framework MUST include benchmarks measuring actor-to-actor message passing latency with both actors pinned to the same NUMA node.
- **FR-011**: The framework MUST include benchmarks measuring actor-to-actor message passing latency with actors pinned to different NUMA nodes.
- **FR-012**: The framework MUST include benchmarks measuring actor-to-actor message passing throughput for same-node and cross-node configurations.
- **FR-013**: Benchmark results MUST be labeled with the NUMA configuration (same-node vs cross-node) for direct comparison.
- **FR-020**: Benchmarks MUST compare NUMA-local allocated channels vs default-allocated channels to quantify the memory locality benefit.

**Example**:

- **FR-014**: The framework MUST include a runnable example demonstrating NUMA topology discovery, actor pinning to specific nodes, message exchange, and latency reporting.

### Key Entities

- **CpuSet**: A set of CPU core IDs representing a thread affinity mask. Specifies which cores a thread is allowed to run on.
- **NumaTopology**: A runtime representation of the system's NUMA layout — the number of nodes and the mapping from node ID to CPU IDs.
- **NumaNode**: A single NUMA node, identified by an integer ID, containing a set of CPU IDs that share local memory.
- **NumaAllocator**: A memory allocator that binds allocations to a specific NUMA node, ensuring data locality for actor-owned state and channel buffers.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: An actor pinned to a specific CPU reports that CPU as its execution processor in 100% of test runs.
- **SC-002**: The NUMA topology query returns correct node-to-CPU mappings that match the system's actual hardware layout.
- **SC-003**: Cross-node actor-to-actor latency is measurably higher (statistically significant) than same-node latency in benchmark results.
- **SC-004**: Actors created without CPU affinity pass all existing tests unchanged — zero backward-compatibility regressions.
- **SC-005**: All public APIs have doc tests and unit tests per project constitution.
- **SC-006**: The NUMA pinning example compiles, runs, and produces correct output on a multi-NUMA system.
- **SC-007**: Benchmarks produce results for at least 2 configurations (same-node, cross-node) and results are directly comparable (same message count, same methodology).
- **SC-008**: NUMA-local allocated channels show measurably lower latency than default-allocated channels when actors are pinned to the same NUMA node as the allocation.

## Assumptions

- All target systems have at least 2 NUMA nodes, as stated in the feature description. The framework will still work on single-NUMA systems but cross-node benchmarks will be skipped or report a warning.
- Thread pinning uses Linux-specific system calls. This feature is Linux-only, consistent with the project constitution.
- CPU affinity is set at thread spawn time (before the message loop). Dynamic re-pinning of a running actor thread is out of scope.
- The NUMA topology is read once at query time and is assumed to remain stable for the process lifetime. Hot-plug CPU changes are out of scope.
- Benchmarks use the existing Criterion framework already present in the project.
- The actor's message handler runs on the pinned thread — any work spawned by the handler onto other threads is not affected by the actor's affinity.
- This feature builds on top of feature 003 (actor-channels). The existing actor creation API is extended with an optional affinity parameter; no breaking changes.

## Clarifications

### Session 2026-03-31

- Q: Should NUMA-awareness include memory allocation in addition to thread pinning? → A: Yes — thread pinning + general NUMA-local allocator for all actor-owned data (handler state, channel buffers, internal buffers).
- Q: Should CPU affinity be fixed at creation or changeable? → A: Mutable between activations. Affinity can be changed while the actor is idle (not running), applied on next activation.
