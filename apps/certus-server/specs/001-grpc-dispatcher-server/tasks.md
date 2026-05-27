# Tasks: gRPC Dispatcher Server

**Input**: Design documents from `/specs/001-grpc-dispatcher-server/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/dispatcher.proto

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create the certus-server crate, configure dependencies, and establish the protobuf build pipeline.

- [x] T001 Create `apps/certus-server/Cargo.toml` with dependencies: tonic, prost, tokio, clap, interfaces (features=["spdk"]), component-framework, component-core, dispatcher, dispatch-map, spdk-env, gpu-services
- [x] T002 Create `apps/certus-server/build.rs` with tonic-build to compile `proto/dispatcher.proto`
- [x] T003 Copy proto contract from `specs/001-grpc-dispatcher-server/contracts/dispatcher.proto` to `apps/certus-server/proto/dispatcher.proto`
- [x] T004 Add `certus-server` to workspace members in root `Cargo.toml`
- [x] T005 [P] Create `apps/certus-server/python-client/requirements.txt` with grpcio, grpcio-tools, protobuf dependencies
- [x] T006 [P] Create `apps/certus-server/python-client/generate_pb.sh` script to generate Python stubs from `../proto/dispatcher.proto`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Implement the gRPC service skeleton, component wiring, and CLI argument parsing that ALL user stories depend on.

**CRITICAL**: No user story work can begin until this phase is complete.

- [x] T007 Implement CLI argument parsing in `apps/certus-server/src/main.rs` using clap derive: `--device-pci` (repeatable), `--listen` (default 0.0.0.0:50051), `--tls-cert`, `--tls-key`
- [x] T008 Implement Certus component stack initialization in `apps/certus-server/src/main.rs` following `certus-connector/src/engine.rs` pattern: SPDKEnv → GpuServices → DispatchMap → Dispatcher wiring and initialization
- [x] T009 Create `apps/certus-server/src/service.rs` with gRPC service struct holding `Arc<std::sync::Mutex<dyn IDispatcher + Send + Sync>>` and implementing the tonic-generated `Dispatcher` trait (empty stubs returning unimplemented)
- [x] T010 Implement tokio runtime setup, gRPC server creation (with optional TLS from CLI args), signal handling (SIGTERM/SIGINT → graceful shutdown), and server startup in `apps/certus-server/src/main.rs`
- [x] T011 Implement batch duplicate-key pre-validation helper function in `apps/certus-server/src/service.rs` that checks a slice of keys for duplicates and returns an error identifying the duplicate key(s)
- [x] T012 Implement `DispatcherError` → protobuf `ErrorCode` mapping helper in `apps/certus-server/src/service.rs`

**Checkpoint**: Server compiles, starts with CLI args, initializes component stack, listens on gRPC port, and shuts down cleanly on SIGTERM. All RPC methods return UNIMPLEMENTED.

---

## Phase 3: User Story 1 - Start Server with Device Configuration (Priority: P1)

**Goal**: Operator can launch server with PCI addresses via CLI and have it ready to accept gRPC connections.

**Independent Test**: Launch server with valid args, confirm it logs readiness and accepts a gRPC connection (Python client connects without error).

### Implementation for User Story 1

- [x] T013 [US1] Add startup logging in `apps/certus-server/src/main.rs`: log PCI addresses being used, component initialization progress, and final "listening on <addr>" message
- [x] T014 [US1] Add CLI validation in `apps/certus-server/src/main.rs`: exit with usage error if `--device-pci` not provided at least once; validate PCI address format
- [x] T015 [US1] Verify build compiles with `cargo build -p certus-server` and fix any dependency issues

**Checkpoint**: Server starts with valid CLI args, logs initialization, reports readiness on port. Exits with clear error on missing/invalid args. Handles SIGTERM/SIGINT gracefully.

---

## Phase 4: User Story 2 - Batch Populate (Priority: P1)

**Goal**: Python client can populate multiple cache entries in a single gRPC call; server iterates entries and returns per-entry results.

**Independent Test**: Python client populates 10 entries in one call, verifies all succeed. Populates with a duplicate key, verifies AlreadyExists returned for that entry.

### Implementation for User Story 2

- [x] T016 [US2] Implement `Populate` RPC handler in `apps/certus-server/src/service.rs`: acquire Mutex, validate no duplicate keys (FR-015), iterate entries calling `dispatcher.populate()` for each, build per-entry `EntryResult` with error mapping, return `BatchPopulateResponse`
- [x] T017 [US2] Implement Python test client populate test in `apps/certus-server/python-client/test_client.py`: connect to server, call `Populate` with a batch of 10 entries, assert all results show success
- [x] T018 [US2] Add duplicate-key rejection test in `apps/certus-server/python-client/test_client.py`: submit batch with same key twice, assert entire batch rejected with DUPLICATE_KEY error
- [x] T019 [US2] Add already-exists test in `apps/certus-server/python-client/test_client.py`: populate a key, then populate same key again in a separate batch, verify per-entry AlreadyExists error

**Checkpoint**: Python client can batch-populate entries. Duplicate keys in a single batch are rejected. Already-existing keys report per-entry error.

---

## Phase 5: User Story 3 - Batch Lookup and Check (Priority: P2)

**Goal**: Python client can check existence and retrieve data for multiple cache entries in a single gRPC call.

**Independent Test**: Populate entries via Populate RPC, then call Check and Lookup in batch, verify correct responses.

### Implementation for User Story 3

- [x] T020 [P] [US3] Implement `Check` RPC handler in `apps/certus-server/src/service.rs`: acquire Mutex, validate no duplicate keys, iterate keys calling `dispatcher.check()` for each, return `BatchCheckResponse` with per-key exists boolean
- [x] T021 [P] [US3] Implement `Lookup` RPC handler in `apps/certus-server/src/service.rs`: acquire Mutex, validate no duplicate keys, iterate entries calling `dispatcher.lookup()` for each, build per-entry `EntryResult`, return `BatchLookupResponse`
- [x] T022 [US3] Add check test in `apps/certus-server/python-client/test_client.py`: populate 5 entries, call Check with those 5 keys plus 5 non-existent keys, assert correct exists/not-exists pattern
- [x] T023 [US3] Add lookup test in `apps/certus-server/python-client/test_client.py`: populate entries, call Lookup with matching ipc_handles, verify per-entry success; lookup non-existent key, verify KeyNotFound

**Checkpoint**: Python client can batch-check and batch-lookup entries. Non-existent keys correctly report false/KeyNotFound.

---

## Phase 6: User Story 4 - Batch Remove (Priority: P2)

**Goal**: Python client can remove multiple cache entries in a single gRPC call.

**Independent Test**: Populate entries, remove them in batch, verify they no longer exist via Check.

### Implementation for User Story 4

- [x] T024 [US4] Implement `Remove` RPC handler in `apps/certus-server/src/service.rs`: acquire Mutex, validate no duplicate keys, iterate keys calling `dispatcher.remove()` for each, build per-entry `EntryResult`, return `BatchRemoveResponse`
- [x] T025 [US4] Add remove test in `apps/certus-server/python-client/test_client.py`: populate 5 entries, remove all 5 in batch, assert all succeed; call Check to confirm gone
- [x] T026 [US4] Add remove-nonexistent test in `apps/certus-server/python-client/test_client.py`: remove keys that don't exist, verify per-entry KeyNotFound

**Checkpoint**: Python client can batch-remove entries. Full lifecycle (populate → check → lookup → remove → check) works end-to-end.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: End-to-end validation, Python client orchestration, documentation.

- [x] T027 [P] Create full lifecycle test in `apps/certus-server/python-client/test_client.py`: populate batch → check all exist → lookup all → remove all → check all gone (SC-001)
- [x] T028 [P] Add large batch test in `apps/certus-server/python-client/test_client.py`: populate 1000 entries, check, lookup, remove — verify no timeout (SC-002)
- [x] T029 Create `apps/certus-server/python-client/test_client.py` main entry point with argparse (--server flag), test orchestration, and pass/fail reporting matching quickstart.md expected output
- [x] T030 Run `cargo clippy -p certus-server -- -D warnings` and fix any warnings
- [x] T031 Verify `cargo doc -p certus-server --no-deps` produces no warnings; add doc comments to public items in `src/main.rs` and `src/service.rs`
- [x] T032 Run quickstart.md validation: start server, run test client, verify expected output matches

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 completion — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Phase 2 — server must compile and run
- **User Story 2 (Phase 4)**: Depends on Phase 2 — needs service skeleton and component wiring
- **User Story 3 (Phase 5)**: Depends on Phase 2 — can run parallel with US2 but tests assume populate works
- **User Story 4 (Phase 6)**: Depends on Phase 2 — can run parallel with US2/US3 but tests assume populate works
- **Polish (Phase 7)**: Depends on Phases 3–6 complete

### User Story Dependencies

- **US1 (Server Start)**: Foundation only — independently testable
- **US2 (Populate)**: Foundation only — independently testable (enables other stories' tests)
- **US3 (Lookup/Check)**: Foundation + US2 test data assumed — implementation is independent, tests exercise populate first
- **US4 (Remove)**: Foundation + US2 test data assumed — implementation is independent, tests exercise populate first

### Within Each User Story

- RPC handler implementation before Python test
- Duplicate-key validation helper (T011) must exist before RPC handlers

### Parallel Opportunities

- T005, T006 (Python client setup) parallel with T001–T004 (Rust setup)
- T020, T021 (Check/Lookup handlers) can be implemented in parallel
- T027, T028 (cross-cutting tests) can be written in parallel
- US3 and US4 RPC implementations can proceed in parallel after foundation

---

## Parallel Example: User Story 3

```bash
# These two tasks modify different functions in the same file but are independent:
Task T020: "Implement Check RPC handler in apps/certus-server/src/service.rs"
Task T021: "Implement Lookup RPC handler in apps/certus-server/src/service.rs"
# Both can be implemented in parallel since they are separate methods
```

---

## Implementation Strategy

### MVP First (User Stories 1 + 2)

1. Complete Phase 1: Setup (T001–T006)
2. Complete Phase 2: Foundational (T007–T012)
3. Complete Phase 3: US1 — server starts and accepts connections (T013–T015)
4. Complete Phase 4: US2 — batch populate works end-to-end (T016–T019)
5. **STOP and VALIDATE**: Python client can connect and populate entries
6. This is a shippable MVP — the write path works

### Incremental Delivery

1. Setup + Foundation → Server runs, returns UNIMPLEMENTED
2. + US1 → Server starts cleanly with logging and validation
3. + US2 → Populate works, Python client demonstrates write path
4. + US3 → Check/Lookup works, read path complete
5. + US4 → Remove works, full CRUD lifecycle complete
6. + Polish → Large-batch validation, documentation, clippy clean

---

## Notes

- All RPC handlers use `spawn_blocking` with Mutex since dispatcher is synchronous (SPDK I/O)
- Duplicate-key pre-validation rejects at gRPC level (Status::INVALID_ARGUMENT) before acquiring dispatcher lock
- Python test client generates stubs via `generate_pb.sh` — must be run once after proto changes
- The proto file in `apps/certus-server/proto/` is the source of truth for both Rust codegen and Python stubs
