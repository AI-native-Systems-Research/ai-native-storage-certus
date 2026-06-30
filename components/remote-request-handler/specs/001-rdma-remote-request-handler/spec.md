# Feature Specification: RDMA Remote Request Handler

**Feature Branch**: `feat/remote-request-handler-component`

**Created**: 2026-06-23

**Status**: Draft

**Input**: User description: "Provide an endpoint for other Certus instances to request remote lookups over an RDMA network, with batched asynchronous lookup, session management, direct memory transfer into caller memory, telemetry, and a test client."

## Clarifications

### Session 2026-06-23

- Q: What is the maximum batch size per lookup request? → A: 64 entries maximum per batch.
- Q: How are incoming connections authenticated/authorized? → A: Network-level trust only (assume isolated RDMA fabric provides the security perimeter).
- Q: What is the format/type of lookup keys? → A: Two distinct keys: a 64-bit CacheKey (identifies the cached object) and a 32-bit RDMA memory key (rkey for remote memory access).
- Q: How are connection failures detected? → A: Rely on RDMA CM disconnect events only; no application-level heartbeat.
- Q: How will the binary protocol handle version evolution? → A: Version field in handshake; reject on mismatch.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Remote Node Requests Batched Lookup (Priority: P1)

A remote Certus node needs to resolve a batch of lookup keys that are not available in its local cache. It connects to this node's remote request handler, submits a batch of up to 64 lookup requests (each identified by a 64-bit CacheKey) along with remote memory addresses and 32-bit RDMA memory keys where results should be written, and receives the results directly into its own memory without intermediate copies.

**Why this priority**: This is the core value proposition — enabling distributed lookups across Certus nodes with minimal latency by leveraging direct memory transfer.

**Independent Test**: Can be fully tested by running a test client that connects to the handler, submits a batch of CacheKeys with memory targets (address + rkey), and verifies that results appear at the specified memory locations.

**Acceptance Scenarios**:

1. **Given** the handler is listening on a configured port, **When** a remote node connects and submits a batch of up to 64 lookup requests with memory addresses and rkeys, **Then** the handler resolves each CacheKey and transfers results directly into the specified remote memory locations.
2. **Given** a batch contains multiple CacheKeys, **When** the handler processes the batch, **Then** all lookups are processed asynchronously and results are delivered without blocking on individual completions.
3. **Given** a CacheKey cannot be resolved, **When** the handler processes that entry, **Then** the caller receives an appropriate error indication for that specific entry without affecting other entries in the batch.
4. **Given** a batch exceeds 64 entries, **When** the handler receives the request, **Then** it rejects the batch with an error indicating the maximum size was exceeded.

---

### User Story 2 - Session Lifecycle Management (Priority: P2)

A remote node establishes a session (connection) to the handler via the connection management channel, which includes a protocol version handshake. It then performs one or more batched lookups over that session, and eventually closes the session cleanly when no longer needed.

**Why this priority**: Session management is essential for resource cleanup, connection pooling, and operational stability — without it, connections would leak.

**Independent Test**: Can be tested by connecting a client, performing lookups, then closing the session, and verifying all resources are released.

**Acceptance Scenarios**:

1. **Given** the handler is running, **When** a remote node initiates a connection on the configured port, **Then** a protocol version handshake occurs and, if versions match, a new session is created and the connection is ready for lookup requests.
2. **Given** a version mismatch during handshake, **When** the remote node connects, **Then** the handler rejects the connection with an error indicating the version incompatibility.
3. **Given** an active session exists, **When** the remote node sends a close-connection request, **Then** the session is terminated and all associated resources are released.
4. **Given** an active session exists, **When** the remote node disconnects unexpectedly (detected via RDMA CM disconnect event), **Then** the handler cleans up the session resources.

---

### User Story 3 - Operator Monitors Connection and Throughput Metrics (Priority: P3)

A system operator enables telemetry on the handler to observe connection rates and data transfer throughput/latency, allowing them to diagnose performance issues and plan capacity.

**Why this priority**: Telemetry is important for production operations but not required for functional correctness — the handler works without it.

**Independent Test**: Can be tested by enabling telemetry, running lookups through the handler, and verifying that connection count, throughput, and latency metrics are recorded.

**Acceptance Scenarios**:

1. **Given** telemetry is enabled, **When** sessions are established and lookups are processed, **Then** the system records connection rates, data copy throughput, and latency measurements.
2. **Given** telemetry is disabled, **When** the handler processes requests, **Then** no performance overhead from metric collection is incurred.

---

### User Story 4 - Test Client Validates Handler Endpoint (Priority: P2)

A developer uses a dedicated test client program to verify the handler is functioning correctly — connecting (with version handshake), performing lookup batches, and disconnecting — without needing to run a full Certus cluster.

**Why this priority**: A standalone test client enables rapid development iteration and integration testing without deploying the full distributed system.

**Independent Test**: Can be tested by running the test client against a handler instance and verifying it connects, sends lookup batches, receives responses, and disconnects successfully.

**Acceptance Scenarios**:

1. **Given** the handler is running, **When** the test client is launched pointing at the handler's address and port, **Then** it connects (completing version handshake), performs a configurable batch of lookups, reports results, and disconnects cleanly.
2. **Given** the handler is not running, **When** the test client attempts to connect, **Then** it reports a clear connection error.

---

### Edge Cases

- What happens when the handler receives a connection request but the configured port is already in use?
- How does the system handle a lookup batch where all entries fail resolution?
- What happens when the remote caller provides an invalid RDMA rkey or memory address (resulting in a failed RDMA write)?
- How does the handler behave when maximum concurrent sessions are reached?
- What happens if the handler's dispatch dependency is unavailable during a lookup?
- What happens if a batch request contains zero entries?
- What happens when a protocol version mismatch is detected during handshake?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST accept incoming connection requests on a configurable network port, separate from any other service ports.
- **FR-002**: System MUST perform a protocol version handshake on each new connection and reject connections with incompatible versions.
- **FR-003**: System MUST establish a dedicated session for each accepted connection, maintaining session state independently.
- **FR-004**: System MUST support a batched lookup operation where a caller submits up to 64 CacheKeys (64-bit identifiers) in a single request, along with corresponding remote memory addresses and 32-bit RDMA memory keys.
- **FR-005**: System MUST reject batch requests that exceed 64 entries with an appropriate error.
- **FR-006**: System MUST process lookup batches efficiently — key resolution is performed serially, then RDMA Writes are posted in a tight serial loop using unsignaled completions (only the last Write is signaled) for maximum NIC pipelining. This achieves non-blocking network behavior via hardware-level pipelining without requiring per-lookup async task spawning.
- **FR-007**: System MUST transfer lookup results directly into caller-specified remote memory locations using the provided addresses and 32-bit RDMA memory keys.
- **FR-008**: System MUST support a close-connection operation that cleanly terminates a session and releases all resources.
- **FR-009**: System MUST detect session failures and clean up associated resources without requiring an application-level heartbeat. Detection occurs via transport-level I/O errors (recv_msg/send_msg failures) causing the session handler to exit and release resources. Explicit RDMA CM disconnect event monitoring (via a dedicated event listener) is defined but not yet wired into the serve loop; it may be added in a future iteration for faster failure detection.
- **FR-010**: System MUST delegate lookup resolution to a dispatch service (initially a placeholder that logs each request).
- **FR-011**: System MUST route all diagnostic and informational output through a logging interface.
- **FR-012**: System MUST include a standalone test client program capable of connecting (with version handshake), submitting batched lookups, and disconnecting.
- **FR-013**: System MUST include unit tests covering session state management, protocol encoding/decoding, listener registry, and RDMA mock interactions. Full end-to-end integration tests (connecting through the serve loop) are not currently implemented — individual module-level tests validate each component in isolation.
- **FR-014**: System MUST optionally collect telemetry on connection rates and data transfer throughput/latency when enabled. The `TelemetryCollector` implementation exists behind `#[cfg(feature = "telemetry")]` but is not yet integrated into the serve loop — wiring telemetry collection into the runtime path is a future task.
- **FR-015**: System MUST be deployable as part of a "full-remote" configuration profile for the server executive. The component is wired into the profile YAML for receptacle binding, but actual RDMA serving is invoked via `serve::run_blocking()` / `serve::bind_listener()` directly in the server main — the `IRemoteRequestHandler` trait methods on the component are stubs (return `NotInitialized`) that exist for framework compatibility rather than functional dispatch.
- **FR-016**: System MUST use a lightweight binary protocol for communication (not HTTP/REST).
- **FR-017**: Security relies on network-level isolation (trusted RDMA fabric); no application-level authentication is required.

### Key Entities

- **Session**: A stateful connection between a remote node and this handler, created after successful version handshake and destroyed on close or RDMA CM disconnect event. Carries per-connection resources.
- **CacheKey**: A 64-bit identifier that uniquely identifies a cached object to be looked up via the dispatch service.
- **RDMA Memory Key (rkey)**: A 32-bit access key provided by the remote caller that authorizes the handler to write result data into the caller's specified memory address.
- **Lookup Batch**: A collection of 1 to 64 lookup requests submitted together, each containing a CacheKey to resolve and a remote memory target (address + rkey) for result delivery.
- **Remote Memory Target**: A descriptor provided by the caller specifying where in the caller's memory the result data should be written, consisting of a memory address and a 32-bit rkey.
- **Dispatch Service**: The subsystem responsible for resolving CacheKeys to result data. Initially a placeholder that logs requests.
- **Protocol Version**: A version identifier exchanged during connection handshake; mismatched versions cause connection rejection.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A remote node can establish a session and complete a batched lookup of 64 entries in under 500 microseconds (excluding network round-trip).
- **SC-002**: The handler supports at least 100 concurrent sessions without resource exhaustion or degraded lookup latency.
- **SC-003**: The test client can successfully connect, perform lookups, and disconnect from the handler with zero errors in a clean environment.
- **SC-004**: Session cleanup on RDMA CM disconnect event releases 100% of associated resources within 1 second.
- **SC-005**: When telemetry is enabled, throughput and latency metrics are available with less than 5% performance overhead compared to telemetry-disabled mode.
- **SC-006**: Protocol version mismatch is detected and connection rejected within one round-trip of the handshake.

## Assumptions

- The handler runs on a system with RDMA-capable network hardware and appropriate kernel/driver support.
- The RDMA fabric is an isolated, trusted network — no application-level authentication is necessary.
- The configurable port number is provided at component initialization time and does not conflict with other services.
- The dispatch service interface (IDispatcher) is available to the handler at session creation; initially this is a stub that logs requests.
- The remote caller is responsible for providing valid memory addresses and RDMA rkeys — the handler trusts these values within the isolated fabric.
- The lightweight binary protocol includes a version field in the handshake; incompatible versions are rejected outright (no negotiation).
- The "full-remote" profile extends the existing certus-server-yaml executive configuration.
- Telemetry collection is opt-in and disabled by default.
- Maximum batch size is 64 entries; callers requiring more must split across multiple requests.
