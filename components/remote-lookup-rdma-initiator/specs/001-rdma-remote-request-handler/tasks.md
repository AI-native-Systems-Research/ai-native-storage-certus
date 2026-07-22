# Tasks: RDMA Remote Lookup Initiator

**Input**: Design documents from `specs/001-rdma-remote-lookup-rdma-initiator/`

**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Tests**: Included — the feature specification requires unit tests (FR-013) and a test client (FR-012).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3, US4)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization, dependency configuration, and code generation setup

- [x] T001 Update Cargo.toml with dependencies: tokio, prost, prost-types, and dev-dependencies in Cargo.toml
- [x] T002 Create proto/remote_request.proto from contracts/remote_request.proto specification
- [x] T003 Create build.rs with prost-build configuration to compile proto/remote_request.proto
- [x] T004 [P] Create source directory structure: src/listener.rs, src/session.rs, src/protocol.rs, src/rdma.rs, src/telemetry.rs (empty modules)
- [x] T005 [P] Update src/lib.rs to declare all new modules and re-export public types

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: RDMA FFI bindings, protocol layer, and base listener that ALL user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T006 Create RDMA FFI bindings module in src/rdma.rs: safe wrappers around rdma_cm (rdma_create_event_channel, rdma_create_id, rdma_bind_addr, rdma_listen, rdma_get_request, rdma_accept, rdma_disconnect) and ibverbs (ibv_create_cq, ibv_create_qp, ibv_reg_mr, ibv_post_send, ibv_post_recv, ibv_poll_cq, ibv_dereg_mr, ibv_destroy_qp, ibv_destroy_cq)
- [x] T007 Implement protocol message encode/decode wrappers in src/protocol.rs using generated prost types (RequestMessage, ResponseMessage envelope handling)
- [x] T008 Implement async connection listener in src/listener.rs: tokio task that polls rdma_cm event channel, accepts new connections, creates QP and posts initial recv buffers
- [x] T009 Implement base Session struct in src/session.rs: holds QP handle, session state enum (Connecting/Handshake/Active/Closing/Closed), session ID, and reference to IDispatcher
- [x] T010 [P] Write unit tests for protocol encode/decode round-trip in src/protocol.rs (tests module)
- [x] T011 [P] Write unit tests for Session state transitions in src/session.rs (tests module)

**Checkpoint**: Foundation ready — RDMA listen, accept, and basic session creation functional with mocked RDMA layer

---

## Phase 3: User Story 1 - Remote Node Requests Batched Lookup (Priority: P1) 🎯 MVP

**Goal**: A remote node can submit a batch of up to 64 CacheKey lookups and receive results written directly into its remote memory via RDMA Write.

**Independent Test**: Submit a batch of CacheKeys via the test client; verify IDispatcher is called for each key and RDMA Writes are issued to the specified remote addresses.

### Implementation for User Story 1

- [x] T012 [US1] Implement handshake processing in src/session.rs: validate protocol_version, send HandshakeResponse with max_batch_size=64, transition state to Active
- [x] T013 [US1] Implement batch lookup request handling in src/session.rs: deserialize BatchLookupRequest, validate batch size ≤ 64, reject oversized batches with ERROR_CODE_BATCH_TOO_LARGE
- [x] T014 [US1] Implement async dispatch resolution in src/session.rs: for each LookupEntry in batch, call IDispatcher to resolve CacheKey, collect results
- [x] T015 [US1] Implement RDMA Write result delivery in src/session.rs: for each resolved entry, issue ibv_post_send with RDMA Write opcode to (remote_addr, rkey), handle write completions from CQ
- [x] T016 [US1] Implement batch response assembly in src/session.rs: construct BatchLookupResponse with per-entry success/failure status and bytes_written, send via RDMA Send
- [x] T017 [US1] Integrate session recv loop: post recv buffers, poll CQ for incoming requests, dispatch to handshake or batch_lookup handler based on RequestMessage oneof
- [x] T018 [P] [US1] Write unit test for batch size validation (reject > 64 entries) in src/session.rs
- [x] T019 [P] [US1] Write unit test for batch lookup with mocked IDispatcher (verify per-key dispatch calls) in src/session.rs
- [x] T020 [P] [US1] Write unit test for batch response assembly (verify correct EntryResult per key) in src/session.rs

**Checkpoint**: Batched lookup functional end-to-end (with mocked RDMA for unit tests). IDispatcher placeholder logs each lookup request.

---

## Phase 4: User Story 2 - Session Lifecycle Management (Priority: P2)

**Goal**: Sessions are created with version handshake, support graceful close, and auto-cleanup on unexpected RDMA CM disconnect events.

**Independent Test**: Connect a client, verify handshake succeeds (and version mismatch rejects), send CloseRequest and verify session resources are released, simulate disconnect and verify cleanup.

### Implementation for User Story 2

- [x] T021 [US2] Implement version mismatch rejection in src/session.rs: if caller version != handler version, send HandshakeResponse with accepted=false and error_message, close connection
- [x] T022 [US2] Implement CloseRequest handling in src/session.rs: transition to Closing state, drain in-flight operations, send CloseResponse with batches_total, destroy QP and deregister MRs
- [x] T023 [US2] Implement RDMA CM disconnect event handling in src/listener.rs: detect RDMA_CM_EVENT_DISCONNECTED, look up session by cm_id, trigger cleanup (transition to Closed, release all resources)
- [x] T024 [US2] Implement session resource cleanup helper in src/session.rs: deregister memory regions, destroy QP, destroy CQ, free recv buffers, remove session from active sessions map
- [x] T025 [US2] Add concurrent session tracking to src/listener.rs: maintain HashMap of active sessions, enforce max_sessions limit (reject new connections when at capacity)
- [x] T026 [P] [US2] Write unit test for version mismatch rejection in src/session.rs
- [x] T027 [P] [US2] Write unit test for graceful close (verify resource cleanup) in src/session.rs
- [x] T028 [P] [US2] Write unit test for max sessions enforcement in src/listener.rs

**Checkpoint**: Full session lifecycle (connect → handshake → active → close/disconnect → cleanup) verified

---

## Phase 5: User Story 4 - Test Client Validates Handler Endpoint (Priority: P2)

**Goal**: A standalone binary program connects to the handler, performs configurable batched lookups, reports results, and disconnects.

**Independent Test**: Run the test client against a running handler instance; verify it completes without errors and reports lookup results.

### Implementation for User Story 4

- [x] T029 [US4] Create src/bin/test_client.rs with clap CLI argument parsing: --addr, --port, --batch-size (default 16), --iterations (default 1), --client-id
- [ ] T030 [US4] Implement client-side RDMA connection setup in src/bin/test_client.rs: rdma_resolve_addr, rdma_resolve_route, rdma_connect, create QP, register memory region for recv buffers and result buffers
- [ ] T031 [US4] Implement handshake exchange in src/bin/test_client.rs: send HandshakeRequest, recv and validate HandshakeResponse (check accepted, report max_batch_size)
- [ ] T032 [US4] Implement batch lookup execution in src/bin/test_client.rs: allocate result buffers, register with RDMA, construct BatchLookupRequest with CacheKeys and (remote_addr, rkey) for each entry, send and recv response
- [ ] T033 [US4] Implement result reporting and graceful disconnect in src/bin/test_client.rs: print per-entry results, send CloseRequest, recv CloseResponse, cleanup RDMA resources
- [ ] T034 [US4] Add error handling for connection failures in src/bin/test_client.rs: report clear error when handler is unreachable, handle handshake rejection

**Checkpoint**: Test client binary can exercise full handler path: connect → handshake → batched lookups → close

---

## Phase 6: User Story 3 - Operator Monitors Metrics (Priority: P3)

**Goal**: Optional telemetry (feature-gated) records connection rates, data transfer throughput, and batch latency via ILogger.

**Independent Test**: Enable telemetry feature, run lookups, verify metrics are logged; disable feature and verify no overhead.

### Implementation for User Story 3

- [x] T035 [US3] Implement telemetry module in src/telemetry.rs: define TelemetryCollector struct with atomic counters for connections_accepted, connections_rejected, active_sessions, batches_processed, entries_resolved, bytes_transferred
- [x] T036 [US3] Add timing instrumentation to src/telemetry.rs: record per-batch start/end timestamps, compute average and p99 batch latency, compute throughput (bytes/sec)
- [x] T037 [US3] Gate telemetry with feature flag: add `telemetry` feature to Cargo.toml, wrap all metric collection in cfg(feature = "telemetry") blocks throughout src/session.rs and src/listener.rs
- [ ] T038 [US3] Implement periodic telemetry reporting via ILogger in src/telemetry.rs: log summary metrics at configurable interval (connection rate, throughput, latency percentiles)
- [x] T039 [P] [US3] Write unit test for TelemetryCollector counter increments in src/telemetry.rs
- [x] T040 [P] [US3] Write compile-time test verifying telemetry code is excluded when feature disabled (build without feature, verify no telemetry symbols)

**Checkpoint**: Telemetry records and reports all specified metrics when enabled, zero overhead when disabled

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Profile integration, documentation, and final validation

- [x] T041 Create profiles/full-remote.yaml in apps/certus-server-yaml/profiles/: extend full profile, add remote_lookup_rdma_initiator component with wiring to logger and dispatcher, add to init_order
- [ ] T042 Add `remote-lookup-rdma-initiator` dependency to apps/certus-server-yaml/Cargo.toml
- [ ] T043 [P] Update IRemoteLookupRdmaInitiator interface in components/interfaces/src/iremote_lookup_rdma_initiator.rs if needed to align with new RDMA-aware batch signature (or add new trait methods)
- [ ] T044 [P] Write integration test in tests/integration/loopback_test.rs: start handler on localhost (SoftRoCE), run test client against it, verify end-to-end flow (gate behind integration-test feature)
- [x] T045 Run cargo fmt --check and cargo clippy -- -D warnings on the full component
- [x] T046 Run cargo doc --no-deps and verify no warnings for public API documentation
- [ ] T047 Validate quickstart.md instructions: build, run test client commands work as documented

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Foundational (Phase 2) — no dependencies on other stories
- **User Story 2 (Phase 4)**: Depends on Foundational (Phase 2) — enhances Session from US1 but independently testable
- **User Story 4 (Phase 5)**: Depends on User Story 1 (Phase 3) — needs a functional handler to test against
- **User Story 3 (Phase 6)**: Depends on Foundational (Phase 2) — can be done in parallel with US1/US2
- **Polish (Phase 7)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational — No dependencies on other stories
- **User Story 2 (P2)**: Can start after Foundational — Extends Session from US1, but lifecycle logic is independently testable
- **User Story 4 (P2)**: Depends on US1 — needs handler to be functional for the client to test against
- **User Story 3 (P3)**: Can start after Foundational — Purely additive (feature-gated), no cross-story dependencies

### Within Each User Story

- Models/types before services/logic
- Core implementation before error handling
- Implementation before tests (tests validate the implementation)
- Story complete before moving to next priority

### Parallel Opportunities

- T004 and T005 can run in parallel (different files, no dependencies)
- T010 and T011 can run in parallel (independent test modules)
- T018, T019, T020 can run in parallel (independent unit tests)
- T026, T027, T028 can run in parallel (independent unit tests)
- T039 and T040 can run in parallel (independent telemetry tests)
- T043 and T044 can run in parallel (interface update vs integration test)
- US1 and US3 can proceed in parallel after Foundational (different modules)

---

## Parallel Example: User Story 1

```bash
# After T013 (batch validation) and T014 (dispatch) complete:
# Launch unit tests in parallel:
Task T018: "Unit test for batch size validation in src/session.rs"
Task T019: "Unit test for batch lookup with mocked IDispatcher in src/session.rs"
Task T020: "Unit test for batch response assembly in src/session.rs"
```

## Parallel Example: Foundational + US3

```bash
# After Phase 2 completes, these can proceed simultaneously:
# Developer A: Phase 3 (US1 - batch lookup core)
# Developer B: Phase 6 (US3 - telemetry module, independent of lookup logic)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001–T005)
2. Complete Phase 2: Foundational (T006–T011)
3. Complete Phase 3: User Story 1 (T012–T020)
4. **STOP and VALIDATE**: Test batch lookup with mocked RDMA + IDispatcher stub logging
5. Handler accepts connections, processes batches, and returns responses

### Incremental Delivery

1. Setup + Foundational → RDMA listener running, sessions accepted
2. Add US1 → Batched lookup works end-to-end (MVP!)
3. Add US2 → Clean close, disconnect cleanup, version rejection
4. Add US4 → Standalone test client for validation
5. Add US3 → Telemetry for production observability
6. Polish → Profile, docs, integration tests

### Single Developer Strategy

Work sequentially in priority order:
1. Phases 1–3 (Setup → Foundation → US1 batch lookup) — core MVP
2. Phase 4 (US2 session lifecycle) — hardening
3. Phase 5 (US4 test client) — validation tooling
4. Phase 6 (US3 telemetry) — observability
5. Phase 7 (Polish) — integration and release readiness

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- RDMA FFI operations are unsafe — each wrapper must include // SAFETY: comments
- Unit tests mock RDMA calls via trait abstractions; integration tests require SoftRoCE
- The IDispatcher placeholder (logging stub) is sufficient for all phases until a real dispatcher is wired
- Feature flag `telemetry` controls US3 compilation — zero-cost when disabled
- Feature flag `integration-test` gates tests requiring RDMA hardware/SoftRoCE
