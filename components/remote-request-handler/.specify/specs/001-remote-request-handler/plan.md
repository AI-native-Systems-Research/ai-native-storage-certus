# Implementation Plan: Remote Request Handler

**Branch**: `001-remote-request-handler` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The Remote Request Handler is an RDMA-based endpoint component that serves cache lookup requests from peer Certus nodes. It listens for RDMA CM connections, performs a protobuf-framed handshake/batch-lookup/close protocol, and writes resolved cache data directly into the caller's registered memory via RDMA Write (zero-copy on the client side). The component follows the Certus component model with a two-phase batch processing strategy that separates lock contention (resolve phase) from NIC I/O posting (write phase) for maximum throughput.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `component-framework` / `component-core` / `component-macros` — Certus component model
- `interfaces` (with `spdk` feature) — `IRemoteRequestHandler`, `ILogger`, `IDispatcher` traits
- `prost` 0.13 / `prost-build` 0.13 — Protobuf serialization (code-gen at build time)
- `tokio` 1.x — Async runtime (session registry uses tokio::sync::Mutex)
- `clap` 4.x — CLI argument parsing for standalone binaries
- `anyhow` 1.x — Error handling with context
- `crc32fast` 1.x — Data integrity verification in test client
- `cc` 1.x — Build-time compilation of `wrapper.c` (C helpers for inline ibverbs functions)
- `pkg-config` 0.3 — Locating system RDMA libraries at build time
- System: `libibverbs`, `librdmacm` — RDMA verbs and connection management

## Architecture

### Component Layer

```
                    +-------------------------------+
                    |   certus-server-yaml          |
                    |   (full-remote profile)       |
                    +------+------------------------+
                           |
                           | bind_listener() + serve_loop()
                           v
+------------+      +------+------------------------+      +------------------+
| ILogger    |<---->| RemoteRequestHandlerComponent |<---->| IDispatcher      |
| (recept.)  |      | implements IRemoteRequestHndlr|     | (receptacle)     |
+------------+      +------+------------------------+      +------------------+
                           |
            +--------------+--------------+
            |              |              |
     +------v------+ +----v------+ +-----v------+
     |  serve.rs   | | session.rs| | listener.rs|
     | serve_loop  | | Session   | | Registry   |
     | bind_listener| | StateMachine| | Config   |
     +-+----+------+ +-----------+ +------------+
       |    |
  +----v-+  +---v--------+
  |rdma.rs|  |protocol.rs |
  |RdmaConn| |proto (prost)|
  |RdmaList| |encode/decode|
  |MemoryReg| +------------+
  +----+---+
       |
  +----v------+     +------------+
  |  ffi.rs   |     |telemetry.rs|
  |ibverbs FFI|     |AtomicU64   |
  |rdmacm FFI |     |feature-gate|
  +----+------+     +------------+
       |
  +----v------+
  | wrapper.c |
  |ibv_post_* |
  |ibv_poll_cq|
  +-----------+
```

### Internal Module Structure

```
src/
  lib.rs              Component definition (define_component!), IRemoteRequestHandler impl (stubs)
  serve.rs            Public server API: bind_listener(), serve_loop(), run_blocking(),
                        handle_session(), process_batch_with_rdma_write() (two-phase)
  session.rs          Session state machine (Connecting->Handshake->Active->Closing->Closed),
                        SessionConfig, SessionError, validate_batch(), process_batch()
  listener.rs         ListenerConfig, SessionRegistry (max-session enforcement), Listener struct
  rdma.rs             RdmaListener (bind/accept/shutdown), RdmaConnection (register_mr, send/recv,
                        rdma_write, rdma_write_from_pool, post_rdma_write_unsignaled, poll_cq),
                        MemoryRegion, RdmaOps trait, MockRdmaOps
  protocol.rs         Protobuf codec: encode/decode helpers, envelope wrappers (handshake, lookup, close)
  ffi.rs              Raw FFI declarations for libibverbs + librdmacm + custom C wrappers
  telemetry.rs        TelemetryCollector (feature-gated atomic counters, zero-cost when disabled)
  wrapper.c           C implementations of inline ibverbs functions (ibv_post_send/recv, ibv_poll_cq)
  bin/
    handler_server.rs Standalone server binary (clap CLI, StderrLogger, calls run_blocking())
    test_client.rs    Test client binary (connects, handshakes, batched lookups, CRC32 verify, close)
proto/
  remote_request.proto  Protobuf schema (v1): Handshake, BatchLookup, Close, ErrorCode, envelopes
build.rs              Protoc download/discovery, prost-build, cc compilation of wrapper.c, pkg-config linking
```

### Data Flow

1. **Connection**: Peer node calls `rdma_resolve_addr` / `rdma_connect`. Server's `RdmaListener::accept()` waits for `RDMA_CM_EVENT_CONNECT_REQUEST`, allocates PD/CQ/QP, calls `rdma_accept`, waits for `RDMA_CM_EVENT_ESTABLISHED`.

2. **Handshake**: Client sends `RequestMessage{Handshake{version, client_id}}` via RDMA Send. Server decodes, validates version, responds with `ResponseMessage{Handshake{accepted, server_version, max_batch_size}}`. On mismatch, session transitions to Closed.

3. **Batch Lookup** (two-phase):
   - **Phase 1 (Resolve)**: For each entry in `BatchLookupRequest`, call `resolver(cache_key)` to get `ResolvedEntry{ptr, size}`. This acquires read references on the dispatch-map, batching lock contention up front.
   - **Phase 2 (Write)**: Post RDMA Writes in a tight loop. All writes are unsignaled except the last (selective signaling via `sq_sig_all=0`). Uses pre-registered pool MR if available (avoids per-entry `ibv_reg_mr`/`ibv_dereg_mr`).
   - **Phase 3 (Release)**: Call `release(cache_key)` for each resolved entry to release read references.
   - Server sends `ResponseMessage{Lookup{batch_id, results[]}}` back via RDMA Send.

4. **Close**: Client sends `RequestMessage{Close{reason}}`. Server responds with `ResponseMessage{Close{batches_total}}` and terminates the session loop.

5. **Shutdown**: External thread calls `RdmaListener::shutdown()` which destroys the event channel (closes the fd), unblocking any pending `rdma_get_cm_event`. The serve_loop detects `is_shutdown()` and exits.

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| Two-phase batch (resolve, then write) | Batches lock contention separately from NIC I/O posting; NIC receives continuous stream of work without stalls |
| Selective signaling (`sq_sig_all=0`) | Only last write generates a CQE, reducing completion processing overhead by `batch_size - 1` |
| Pre-registered pool MR | Eliminates per-entry `ibv_reg_mr`/`ibv_dereg_mr` syscall overhead; single MR for entire memory-tier pool |
| Manual FFI instead of bindgen | Keeps build fast (no bindgen dependency), only exposes the exact symbols needed, allows custom struct layouts |
| `wrapper.c` for inline ibverbs functions | `ibv_post_send`, `ibv_post_recv`, `ibv_poll_cq` are inline in libibverbs headers; cannot be called directly from Rust FFI |
| Component trait stubs + separate serve path | Component model provides the interface contract; `serve.rs` bypasses trait dispatch overhead for the hot path using raw function callbacks |
| Spin-loop CQ polling with timeout | Avoids kernel involvement (no interrupt coalescing needed), lowest latency for completion detection |
| Protobuf over RDMA Send/Recv | Control messages are small; protobuf provides schema evolution. Bulk data goes through RDMA Write (zero-copy). |
| Shutdown via event-channel destruction | More reliable than `rdma_destroy_id` async events; guaranteed to wake blocked `rdma_get_cm_event` on all rdma-core versions |
| Feature-gated telemetry | Zero-cost when disabled (compiles to empty struct); no runtime penalty for production builds without metrics |

## Dependencies

| From | To | Nature |
|------|----|--------|
| `serve.rs` | `rdma.rs` | Uses `RdmaListener`, `RdmaConnection`, `MemoryRegion` |
| `serve.rs` | `protocol.rs` | Encodes/decodes protobuf messages |
| `serve.rs` | `session.rs` | Creates `Session`, calls `process_handshake`, `validate_batch`, `record_batch` |
| `serve.rs` | `ffi.rs` | Device enumeration in `log_rdma_devices()` |
| `rdma.rs` | `ffi.rs` | All ibverbs/rdmacm FFI calls |
| `protocol.rs` | `prost` (generated) | Includes generated code from `remote_request.proto` |
| `listener.rs` | `session.rs` | `SessionRegistry` holds `Arc<Session>` |
| `lib.rs` | `interfaces` | `IRemoteRequestHandler`, `ILogger`, `IDispatcher`, `CacheKey`, `LookupRef` |
| `build.rs` | `prost-build` | Protobuf code generation |
| `build.rs` | `cc` | Compiles `wrapper.c` |
| Linked at runtime | `libibverbs.so`, `librdmacm.so` | RDMA kernel verbs and connection management |

## Testing

| Category | Coverage | Notes |
|----------|----------|-------|
| Session state machine | Unit tests in `session.rs` | Valid/invalid transitions, handshake accept/reject, batch validation, close |
| Protocol roundtrip | Unit tests in `protocol.rs` | All message types (handshake, lookup, close) encode/decode correctly |
| Mock RDMA operations | Unit tests in `rdma.rs` | `MockRdmaOps` records writes/sends, drains completions |
| Session registry | Async unit tests in `listener.rs` | Max-session enforcement, register/remove |
| Telemetry counters | Unit tests in `telemetry.rs` (feature-gated) | Counter increments, throughput calculation |
| Component instantiation | Unit tests in `lib.rs` | Basic creation, batch lookup returns per-key errors |
| Integration (hardware) | `handler-server` + `test-client` binaries | Full RDMA path with CRC32 verification; requires RDMA NIC |
| End-to-end | `certus-server-yaml` full-remote profile + Python client | >200 GB/s throughput target |

## Future Considerations

- **IRemoteRequestHandler trait implementation**: Currently stubbed in `lib.rs`. The trait methods could be connected to the `serve.rs` resolve/release path for non-RDMA callers.
- **Async session handling**: The current serve_loop is synchronous (one session at a time). Multi-session concurrency would require threading or async accept with per-session tasks.
- **Protocol version negotiation**: Currently hard-rejects mismatches. Future versions could negotiate capabilities.
- **Adaptive batch size**: Server could dynamically adjust `max_batch_size` based on QP depth utilization.
- **Connection health monitoring**: Detect CM disconnect events during active sessions (not just at accept time).
- **Metrics export**: The TelemetryCollector could expose metrics via a prometheus endpoint or shared-memory counters.
- **GDRCopy path**: For GPU-resident cache entries, an alternative write path using GDRCopy could avoid PCIe round-trips.
