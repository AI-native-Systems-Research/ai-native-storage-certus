# Research: RDMA Remote Request Handler

## R1: RDMA Connection Management (rdma_cm)

**Decision**: Use `rdma_cm` (librdmacm) for connection setup/teardown with RC (Reliable Connected) transport.

**Rationale**: rdma_cm provides a TCP-like connection semantic over RDMA — the listener binds to a TCP port, accepts connections, and creates RC queue pairs automatically. This matches the spec's requirement for a port-based connection bootstrap. RC guarantees in-order, reliable delivery for the request/response protocol.

**Alternatives considered**:
- UD (Unreliable Datagram): Lower overhead but no connection state, no RDMA Write support, and message size limited to MTU. Rejected — spec requires RDMA Write into caller memory.
- Raw ibverbs without rdma_cm: Manual QP setup is possible but requires out-of-band exchange of QP numbers, LIDs, and GIDs. Rejected — rdma_cm handles this transparently.

## R2: Data Transfer Mechanism

**Decision**: Use RDMA Send/Recv for the request/response protocol messages and RDMA Write for transferring lookup result data directly into caller memory.

**Rationale**: The caller provides remote memory addresses and rkeys in the lookup request. After resolving a CacheKey via IDispatcher, the handler issues an RDMA Write to deposit the data at the caller's specified address using the provided rkey. The completion status is returned via a Send/Recv response message. This achieves zero-copy on the caller side.

**Alternatives considered**:
- Send/Recv only (caller copies from receive buffer): Adds an extra memory copy on the caller side. Rejected — spec explicitly requires direct memory transfer.
- RDMA Read (caller pulls): Would invert the flow — caller would need handler-side memory addresses. Rejected — spec says handler pushes data.

## R3: Protocol Serialization (Protobuf)

**Decision**: Use Protocol Buffers (prost) for request/response message serialization on the control path (Send/Recv).

**Rationale**: The DESIGN.md explicitly calls for protobuf. prost is the standard Rust protobuf library, integrates with prost-build for code generation, and produces compact binary messages. The existing certus-server-yaml already uses prost/tonic, so tooling is established.

**Alternatives considered**:
- FlatBuffers: Zero-copy deserialization, but no existing project usage and more complex schemas. Rejected.
- Custom binary format: Maximum performance but no schema evolution, harder to maintain. Rejected — protobuf version field in handshake provides forward compatibility.
- Cap'n Proto: Zero-copy but smaller ecosystem in Rust. Rejected.

## R4: Async Runtime Integration

**Decision**: Use tokio for the connection listener thread and per-session event loops.

**Rationale**: The DESIGN.md specifies tokio. The certus-server-yaml already depends on tokio with full features. The listener will run in a dedicated tokio task, spawning a new task per accepted connection. RDMA completion polling can be integrated via tokio's `spawn_blocking` or a custom `AsyncFd` wrapper around the completion channel fd.

**Alternatives considered**:
- Dedicated OS threads per session (no async): Simpler RDMA integration but doesn't scale to 100 sessions efficiently. Could be revisited if tokio integration proves too complex.
- smol/async-std: Less ecosystem support in the project. Rejected.

## R5: RDMA Rust Bindings

**Decision**: Create thin unsafe wrappers around rdma-core C library (librdmacm + libibverbs) via bindgen or manual FFI.

**Rationale**: No production-quality Rust RDMA crate exists in the ecosystem. The project already follows this pattern with spdk-sys (bindgen for SPDK C libraries). A local `rdma-sys` or inline FFI module provides the needed subset: `rdma_create_event_channel`, `rdma_create_id`, `rdma_bind_addr`, `rdma_listen`, `rdma_get_request`, `rdma_accept`, `ibv_post_send`, `ibv_post_recv`, `ibv_poll_cq`, `ibv_reg_mr`.

**Alternatives considered**:
- rdma crate (crates.io): Unmaintained, incomplete API coverage. Rejected.
- Full bindgen of all rdma-core headers: Overkill — only need ~20 functions. A focused manual FFI module is more maintainable.

## R6: Testing Strategy

**Decision**: Unit tests mock RDMA operations; integration tests use SoftRoCE (rxe kernel module) or are gated behind a feature flag for hardware testing.

**Rationale**: CI runs on ubuntu-latest without RDMA hardware. SoftRoCE provides a software RDMA device over any Ethernet interface, enabling loopback testing. Hardware-dependent tests are gated behind a `rdma-hw` feature flag (similar to the SPDK crate's approach). The test-client binary doubles as the integration test driver.

**Alternatives considered**:
- Skip integration tests entirely: Missed bugs in RDMA path. Rejected.
- Require hardware in CI: Not feasible on GitHub Actions. Rejected.

## R7: Session State Machine

**Decision**: Sessions follow a simple linear lifecycle: Connecting → Handshake → Active → Closing → Closed.

**Rationale**: The protocol is request-response with no complex negotiation beyond the version handshake. A linear state machine is sufficient and easier to reason about. Unexpected disconnects (detected via CM events) transition directly to Closed from any state.

**Alternatives considered**:
- Reconnection support: Would add Reconnecting state. Rejected per clarification — RDMA CM events are sufficient, no reconnect needed.

## R8: Telemetry Approach

**Decision**: Feature-gated (`telemetry` feature flag) atomic counters and timing measurements, exposed via the ILogger interface.

**Rationale**: Follows the pattern established by block-device-spdk-nvme which uses a `telemetry` feature flag. Metrics include: connections accepted/rejected, active sessions, batches processed, entries resolved, RDMA writes completed, average/p99 batch latency, throughput (bytes/sec). When disabled, zero overhead (compile-time elimination).

**Alternatives considered**:
- Always-on metrics: Adds overhead even when not needed. Rejected per spec ("less than 5% overhead when enabled" implies disabled is zero).
- External metrics framework (prometheus): Too heavyweight for an internal component. May be added later as an adapter.

## R9: Profile Integration

**Decision**: Create `profiles/full-remote.yaml` in certus-server-yaml that extends the `full` profile by adding the `remote_request_handler` component with wiring to logger and dispatcher.

**Rationale**: The existing profile system uses YAML files declaring components, wiring, and init order. The new profile includes everything from `full` plus the remote request handler component, wired to receive ILogger and IDispatcher.

**Alternatives considered**:
- Add to existing `full.yaml`: Would force all deployments to include the RDMA listener. Rejected — should be opt-in.
- Runtime flag instead of profile: Profiles are the established pattern in this project. Rejected.
