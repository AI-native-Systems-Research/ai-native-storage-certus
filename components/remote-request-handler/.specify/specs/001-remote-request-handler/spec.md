# Feature Specification: Remote Request Handler

**Feature Branch**: `001-remote-request-handler`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The Remote Request Handler is an RDMA-based endpoint component for the Certus storage system that handles incoming cache lookup requests from peer Certus nodes. A remote node connects via `rdma_cm`, submits batched lookup requests over a protobuf protocol, and receives resolved data written directly into its registered memory via RDMA Write -- achieving zero-copy data transfer on the client side.

The component follows the Certus component model (`define_component!` / `IRemoteRequestHandler` interface) and integrates with the dispatcher via a receptacle binding. It manages per-connection session state machines, enforces protocol versioning and batch size limits, and provides optional feature-gated telemetry. A two-phase batch processing strategy (resolve all keys, then post all RDMA Writes) maximizes NIC utilization by batching lock contention separately from I/O posting.

## User Scenarios & Testing

### User Story 1 - Remote Cache Lookup (Priority: P1)

As a Certus peer node, I want to request cached objects from another Certus node via RDMA, so that I can retrieve data with minimal latency and zero-copy semantics.

**Acceptance Scenarios**:

- **Given** a running handler server and an RDMA-connected client, **when** the client sends a HandshakeRequest with a matching protocol version, **then** the handler responds with `accepted=true` and advertises `max_batch_size=64`.
- **Given** an active session, **when** the client sends a BatchLookupRequest with cache keys that exist locally, **then** the handler resolves each key via the dispatcher, performs RDMA Write into the client's registered buffers, and returns a BatchLookupResponse with `success=true` and `bytes_written` for each entry.
- **Given** an active session, **when** the client sends a BatchLookupRequest with cache keys that do not exist locally, **then** the handler returns `success=false` with `ErrorCode::KeyNotFound` for each missing entry (no RDMA Write performed for those entries).
- **Given** an active session, **when** the client sends a CloseRequest, **then** the handler responds with the total number of batches processed and terminates the session.

### User Story 2 - Protocol Version Enforcement (Priority: P1)

As the handler operator, I want incompatible clients to be rejected during handshake, so that protocol mismatches are detected early.

**Acceptance Scenarios**:

- **Given** a client connecting with `protocol_version=99` (mismatched), **when** the handshake is processed, **then** the handler responds with `accepted=false`, an error message indicating the version mismatch, and transitions the session to `Closed`.

### User Story 3 - Batch Size Enforcement (Priority: P1)

As the handler, I want to reject oversized or empty batches, so that resource limits are enforced.

**Acceptance Scenarios**:

- **Given** an active session, **when** the client sends a batch with more than 64 entries, **then** the handler returns an error response with `ErrorCode::BatchTooLarge`.
- **Given** an active session, **when** the client sends a batch with zero entries, **then** the handler returns an error (EmptyBatch).

### User Story 4 - Session Limit Enforcement (Priority: P2)

As the handler operator, I want to limit the number of concurrent sessions, so that RDMA resources are bounded.

**Acceptance Scenarios**:

- **Given** the maximum session count is reached, **when** a new client attempts to connect, **then** the handler rejects the connection with `ResourceExhausted`.

### User Story 5 - Graceful Shutdown (Priority: P2)

As the handler operator, I want to gracefully stop the listener, so that in-flight sessions can drain and resources are released.

**Acceptance Scenarios**:

- **Given** the listener is running, **when** `shutdown()` is called, **then** the accept loop exits, the listener channel is destroyed, and any blocked `rdma_get_cm_event` returns.

### User Story 6 - Telemetry Collection (Priority: P3)

As the handler operator, I want optional metrics on connection rates, throughput, and latency, so that I can monitor system health.

**Acceptance Scenarios**:

- **Given** the `telemetry` feature is enabled, **when** connections are accepted/rejected and batches are processed, **then** atomic counters track connections_accepted, connections_rejected, batches_processed, entries_resolved, entries_failed, bytes_transferred, and total_batch_duration_us.
- **Given** the `telemetry` feature is disabled, **then** the `TelemetryCollector` compiles to a zero-cost no-op struct.

### User Story 7 - Memory Pool Pre-Registration (Priority: P2)

As the handler integrator, I want to pre-register the entire memory-tier pool as a single RDMA Memory Region, so that per-entry MR registration/deregistration overhead is eliminated.

**Acceptance Scenarios**:

- **Given** a `PoolRegion` is supplied, **when** a session is established, **then** the handler registers the pool as a single MR and uses `rdma_write_from_pool` for data transfers (zero per-entry MR overhead).
- **Given** no `PoolRegion` is supplied, **when** a lookup resolves, **then** the handler falls back to per-entry `register_existing_mr` + `rdma_write` (higher overhead, still functional).

### User Story 8 - Data Integrity (Priority: P1)

As a Certus client, I want to verify that RDMA-written data matches the original cached content, so that transport correctness is guaranteed.

**Acceptance Scenarios**:

- **Given** the test client is run with `--verify`, **when** data is written via RDMA, **then** the client computes CRC32 over the received buffer and compares it to the expected fill pattern, reporting PASS or FAIL.

## Requirements

### Functional Requirements

- **FR-001**: The component SHALL implement `IRemoteRequestHandler` with methods `handle_lookup`, `handle_check`, `handle_batch_lookup`, and `release_lookup`.
- **FR-002**: The handler SHALL listen for RDMA CM connection requests on a configurable address and port (default `0.0.0.0:18515`).
- **FR-003**: The handler SHALL enforce a protobuf-based protocol with three message types: Handshake, BatchLookup, and Close, multiplexed via a `oneof` envelope (`RequestMessage`/`ResponseMessage`).
- **FR-004**: The handler SHALL validate protocol version during handshake and reject mismatched clients with an informative error.
- **FR-005**: The handler SHALL accept batched lookups of 1-64 entries per request (configurable max, hard-coded default 64).
- **FR-006**: The handler SHALL reject batches exceeding `max_batch_size` or containing zero entries.
- **FR-007**: For each resolved key, the handler SHALL perform an RDMA Write directly into the caller's registered memory buffer (identified by `remote_addr` + `rkey`).
- **FR-008**: The handler SHALL use a two-phase batch strategy: (1) resolve all keys and hold read references, (2) post all RDMA Writes (unsignaled except the last), (3) release all read references.
- **FR-009**: The handler SHALL track per-session batch counts and return the total on close.
- **FR-010**: Session state SHALL follow the state machine: Connecting -> Handshake -> Active -> Closing -> Closed, with defined valid transitions and error on invalid transitions.
- **FR-011**: The handler SHALL support a `force_close()` mechanism for abrupt disconnection (e.g., on CM disconnect events).
- **FR-012**: The handler SHALL provide `bind_listener` (returns listener handle) and `serve_loop` (blocking accept loop) as separate APIs for embedding in larger binaries.
- **FR-013**: The handler SHALL support graceful shutdown via `RdmaListener::shutdown()` which destroys the event channel to unblock any pending `rdma_get_cm_event`.

### Non-Functional Requirements

- **NFR-001**: Per-entry amortized latency SHALL be less than 2 microseconds on ConnectX-6 class hardware with loopback.
- **NFR-002**: The handler SHALL operate in a network-trust security model (no application-level authentication); it assumes an isolated RDMA fabric.
- **NFR-003**: All RDMA resources (QP, MR, CQ, PD) SHALL be cleaned up via RAII (Drop implementations) to prevent resource leaks.
- **NFR-004**: Telemetry collection SHALL be zero-cost when the `telemetry` feature is disabled (compiled out entirely).
- **NFR-005**: The component SHALL build and pass unit tests without RDMA hardware by using mock RDMA operations (`RdmaOps` trait + `MockRdmaOps`).
- **NFR-006**: All unsafe code SHALL include `// SAFETY:` justification comments.
- **NFR-007**: The handler SHALL use spin-loop polling for CQ completions with a configurable timeout (default 10 seconds) and retry logic (max 3 attempts).
- **NFR-008**: The protobuf encoding/decoding SHALL be validated via roundtrip unit tests for all message types.

## Key Entities

| Entity | Description |
|--------|-------------|
| `RemoteRequestHandlerComponent` | Main component struct implementing `IRemoteRequestHandler`, declaring receptacles for `ILogger` and `IDispatcher` |
| `RdmaListener` | RDMA CM listener that binds to an address/port and accepts incoming connections |
| `RdmaConnection` | A single RDMA connection with QP, MR, CQ management and send/recv/write operations |
| `MemoryRegion` | A registered RDMA memory region (either owned buffer or borrowed external pointer) |
| `Session` | Per-connection state machine managing lifecycle (Connecting -> Handshake -> Active -> Closing -> Closed) |
| `SessionRegistry` | Concurrent registry tracking active sessions with max-session enforcement |
| `ListenerConfig` | Configuration for port, max sessions, and protocol version |
| `SessionConfig` | Per-session configuration (protocol version, max batch size) |
| `TelemetryCollector` | Feature-gated atomic counters for operational metrics |
| `Resolver` | Function type `Fn(u64) -> Option<ResolvedEntry>` mapping cache keys to memory-tier pointers |
| `ReleaseCallback` | Function type `Fn(u64)` called after RDMA Write to release read references |
| `PoolRegion` | Descriptor for a pre-registerable memory-tier pool (base pointer + size) |
| `ResolvedEntry` | Pointer + size pair for a resolved cache entry in the memory-tier pool |
| `RdmaOps` (trait) | Abstraction for RDMA operations enabling testability via `MockRdmaOps` |
| `proto::RequestMessage` | Protobuf envelope with `oneof` for Handshake, BatchLookup, or Close request |
| `proto::ResponseMessage` | Protobuf envelope with `oneof` for Handshake, BatchLookup, or Close response |

## Dependencies

| Dependency | Type | Purpose |
|-----------|------|---------|
| `component-framework` | Workspace crate | `define_component!` macro, IUnknown, receptacle binding |
| `interfaces` (with `spdk` feature) | Workspace crate | `IRemoteRequestHandler`, `ILogger`, `IDispatcher`, `CacheKey`, `LookupRef` |
| `libibverbs` / `librdmacm` | System library (FFI) | RDMA verbs and connection management |
| `prost` / `prost-build` | External crate | Protobuf serialization/deserialization and code generation |
| `tokio` | External crate | Async runtime for session management and synchronization primitives |
| `clap` | External crate | CLI argument parsing for server/client binaries |
| `anyhow` | External crate | Error handling with context |
| `crc32fast` | External crate | Data integrity verification in test client |
| `cc` | Build dependency | Compilation of `wrapper.c` (C helpers for inline ibverbs functions) |
| `pkg-config` | Build dependency | Locating system RDMA libraries |

## Success Criteria

1. Unit tests pass without RDMA hardware (`cargo test -p remote-request-handler`) covering session state machine, protocol roundtrips, batch validation, registry limits, and mock RDMA operations.
2. Integration test (handler-server + test-client) completes on RDMA-capable hardware with all batches processed successfully and matching batch counts on close.
3. Data integrity verification (`--verify`) passes CRC32 checks on all RDMA-written entries.
4. Per-entry amortized latency is below 2 microseconds on ConnectX-6 class hardware (loopback).
5. The component integrates with `certus-server-yaml` (full-remote profile) and serves lookup requests from the Python end-to-end test at >200 GB/s throughput.
6. `cargo clippy -p remote-request-handler -- -D warnings` and `cargo fmt -p remote-request-handler --check` pass cleanly.
7. All RDMA resources are freed on session close / listener shutdown (no leaks observable via rdma-core debug logging).

## Implementation Notes

- The `IRemoteRequestHandler` trait methods (`handle_lookup`, `handle_check`, `handle_batch_lookup`) are currently stubbed with `NotInitialized` errors in the component struct. The actual RDMA serving path is in `serve.rs` which uses a `Resolver` function callback pattern rather than going through the component trait directly. This is intentional: the component model provides the interface contract, while the serve module provides the high-performance serving path that bypasses trait dispatch overhead.
- The `wrapper.c` file provides C implementations of `ibv_post_send`, `ibv_post_recv`, `ibv_poll_cq` as these are typically inline functions in libibverbs headers that cannot be called directly from Rust FFI.
- The two-phase batch strategy (Phase 1: resolve all, Phase 2: post writes unsignaled except last, Phase 3: release) is critical for performance. It ensures the NIC work queue stays full without stalls between entries, and lock contention on the dispatch-map is batched rather than interleaved with I/O.
- Protocol version is currently hardcoded to `1`. The handshake mechanism supports future protocol evolution.
- The `sq_sig_all = 0` QP configuration enables selective signaling: only the last RDMA Write in a batch generates a CQE, reducing completion processing overhead.
- The component depends on `interfaces` with the `spdk` feature enabled, coupling it to the SPDK-dependent portion of the workspace. It must be built explicitly with `-p remote-request-handler`.
