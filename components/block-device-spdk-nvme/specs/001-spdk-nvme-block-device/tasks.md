# Tasks: SPDK NVMe Block Device Component

**Input**: Design documents from `specs/001-spdk-nvme-block-device/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/iblock_device.md, quickstart.md

**Tests**: Tests are MANDATORY per Constitution Principle II (Comprehensive Testing). Unit tests, doc tests, and integration tests are included for every user story. Criterion benchmarks are included in the Polish phase per Principle III.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **Source**: `src/` at component root
- **Tests**: `tests/` at component root
- **Benchmarks**: `benches/` at component root

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization, Cargo.toml, and basic source structure

- [X] T001 Create `Cargo.toml` with workspace membership, edition 2021, MSRV 1.75, dependencies on `component-framework`, `component-core`, `component-macros`, `spdk-sys`, `spdk-env`, `interfaces` (with `spdk` feature), `example-logger`; dev-dependencies on `criterion`; optional `telemetry` feature; bench harness entries for `latency` and `throughput`
- [X] T002 Register `components/block-device-spdk-nvme` in workspace `../../Cargo.toml` as a member (excluded from `default-members` since it requires SPDK)
- [X] T003 [P] Create `src/error.rs` with extended `BlockDeviceError` variants (`FeatureNotEnabled`, `NotInitialized`, `Timeout`, `Aborted`, `InvalidNamespace`, `NotSupported`) and `Display`/`Error` impls with doc comments and doc tests
- [X] T004 [P] Create `src/command.rs` with `Command` enum (ReadSync, WriteSync, ReadAsync, WriteAsync, WriteZeros, BatchSubmit, AbortOp, NsProbe, NsCreate, NsFormat, NsDelete, ControllerReset), `Completion` enum (ReadDone, WriteDone, WriteZerosDone, AbortAck, Timeout, NsProbeResult, NsCreated, NsFormatted, NsDeleted, ResetDone, Error), `ControlMessage` enum (ConnectClient, DisconnectClient), and `ClientChannels` struct; all with doc comments and doc tests
- [X] T005 [P] Create `src/telemetry.rs` with `TelemetrySnapshot` struct and feature-gated `TelemetryStats` collector using atomic counters; doc comments and doc tests for the public `TelemetrySnapshot` type

**Checkpoint**: `cargo build` succeeds with the skeleton structure.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core infrastructure that MUST be complete before ANY user story can be implemented

**CRITICAL**: No user story work can begin until this phase is complete

- [X] T006 Create `src/controller.rs` with `NvmeController` struct wrapping SPDK NVMe probe/attach, controller info getters (numa_node, version, max_transfer_size, max_queue_depth, num_io_queues), and safe Drop for controller detach; all unsafe blocks with `// SAFETY:` comments; doc comments and doc tests
- [X] T007 [P] Create `src/qpair.rs` with `QueuePairPool` managing multiple `spdk_nvme_qpair` at different depths, allocation/deallocation, and a `select_qpair(batch_size: usize) -> &mut QueuePair` heuristic; doc comments and doc tests
- [X] T008 [P] Create `src/namespace.rs` with functions for namespace probe (`spdk_nvme_ctrlr_get_ns`), create, format, and delete via SPDK admin commands; `NvmeNamespaceInfo` struct; doc comments and doc tests
- [X] T009 Create `src/actor.rs` with `BlockDeviceHandler` implementing `ActorHandler<ControlMessage>` — client session `Vec<ClientSession>`, `on_start`/`on_stop` lifecycle hooks, main `handle()` that dispatches `ConnectClient`/`DisconnectClient` and then polls all client ingress channels via `try_recv()`; doc comments and doc tests
- [X] T010 Create `src/lib.rs` with `define_component!` for `BlockDeviceSpdkNvmeComponent` providing `IBlockDevice` interface with receptacles `logger: ILogger` and `spdk_env: ISPDKEnv`; implement `IBlockDevice` trait methods (`connect_client`, `sector_size`, `num_sectors`, `max_queue_depth`, `num_io_queues`, `max_transfer_size`, `block_size`, `numa_node`, `nvme_version`, `telemetry`); component instantiation wires controller, creates Actor with NUMA-pinned CPU affinity, activates actor; doc comments and doc tests
- [X] T011 [P] Create `tests/integration.rs` with helper functions for component wiring (create `LoggerComponent`, `SPDKEnvComponent`, wire receptacles, create `BlockDeviceSpdkNvmeComponent`) and a basic smoke test that wires all components and queries `IBlockDevice`

**Checkpoint**: Foundation ready — component can be instantiated, actor spawned, clients connected. User story implementation can begin.

---

## Phase 3: User Story 1 — Basic Block IO Operations (Priority: P1)

**Goal**: Clients perform synchronous read, write, and write-zeros operations against an NVMe namespace with data integrity verification.

**Independent Test**: Write a known pattern to LBAs, read back, verify data matches.

### Tests for User Story 1

- [X] T012 [P] [US1] Unit tests for sync read/write dispatch in `src/actor.rs`: verify that `Command::ReadSync` and `Command::WriteSync` produce correct `Completion::ReadDone` and `Completion::WriteDone` on the callback channel
- [X] T013 [P] [US1] Unit tests for write-zeros dispatch in `src/actor.rs`: verify `Command::WriteZeros` produces `Completion::WriteZerosDone`
- [X] T014 [P] [US1] Unit test for out-of-range LBA read: verify `BlockDeviceError` is returned without crash
- [X] T015 [P] [US1] Doc tests for `Command::ReadSync`, `Command::WriteSync`, `Command::WriteZeros` in `src/command.rs`

### Implementation for User Story 1

- [X] T016 [US1] Implement sync read handling in `src/actor.rs`: on `Command::ReadSync`, call `spdk_nvme_ns_cmd_read` via controller wrapper, poll completions with `spdk_nvme_qpair_process_completions`, send `Completion::ReadDone` on callback channel
- [X] T017 [US1] Implement sync write handling in `src/actor.rs`: on `Command::WriteSync`, call `spdk_nvme_ns_cmd_write` via controller wrapper, poll completions, send `Completion::WriteDone`
- [X] T018 [US1] Implement write-zeros handling in `src/actor.rs`: on `Command::WriteZeros`, call `spdk_nvme_ns_cmd_write_zeroes`, poll completions, send `Completion::WriteZerosDone`
- [X] T019 [US1] Implement LBA bounds checking in `src/actor.rs`: validate `lba + num_blocks <= ns.num_sectors` before submitting, return `Completion::Error` with `BlockDeviceError` for out-of-range
- [X] T020 [US1] Integration test in `tests/integration.rs`: write-read roundtrip — write known pattern via `Command::WriteSync`, read back via `Command::ReadSync`, verify data integrity

**Checkpoint**: User Story 1 is fully functional and independently testable.

---

## Phase 4: User Story 2 — Asynchronous IO with Timeout and Abort (Priority: P1)

**Goal**: Clients submit async read/write operations with timeouts; completions arrive via callback channel; clients can abort in-flight operations.

**Independent Test**: Submit async writes, verify callback completions within timeout; test timeout expiry; test abort.

### Tests for User Story 2

- [X] T021 [P] [US2] Unit tests for async operation handle assignment in `src/actor.rs`: verify unique monotonic `u64` handles returned in completions
- [X] T022 [P] [US2] Unit tests for timeout enforcement in `src/actor.rs`: submit operation with short timeout, verify `Completion::Timeout` delivered within 10% margin
- [X] T023 [P] [US2] Unit tests for abort in `src/actor.rs`: submit async op, abort by handle, verify `Completion::AbortAck`
- [X] T024 [P] [US2] Doc tests for `Command::ReadAsync`, `Command::WriteAsync`, `Command::AbortOp` in `src/command.rs`

### Implementation for User Story 2

- [X] T025 [US2] Implement `PendingOp` tracking in `src/actor.rs`: `HashMap<u64, PendingOp>` per `ClientSession`, monotonic `u64` handle counter, deadline from `Instant::now() + Duration::from_millis(timeout_ms)`
- [X] T026 [US2] Implement async read/write submission in `src/actor.rs`: on `Command::ReadAsync`/`Command::WriteAsync`, submit to SPDK via queue pair, create `PendingOp`, return handle in eventual `Completion::ReadDone`/`Completion::WriteDone`
- [X] T027 [US2] Implement timeout checking in actor poll loop in `src/actor.rs`: each poll cycle, scan all `PendingOp` entries, send `Completion::Timeout` for expired operations, remove from pending map
- [X] T028 [US2] Implement abort handling in `src/actor.rs`: on `Command::AbortOp`, look up handle in pending map, call `spdk_nvme_ctrlr_cmd_abort` if still in-flight, send `Completion::AbortAck`, remove from pending map
- [X] T029 [US2] Integration test in `tests/integration.rs`: submit async write, receive completion callback, verify handle matches

**Checkpoint**: User Stories 1 AND 2 should both work independently.

---

## Phase 5: User Story 3 — Batch Operations (Priority: P2)

**Goal**: Clients submit batches of IO operations; component selects optimal queue pair based on batch size; all completions reported.

**Independent Test**: Submit batch of N writes, verify N completion callbacks; measure throughput exceeds sequential.

### Tests for User Story 3

- [X] T030 [P] [US3] Unit tests for queue pair selection heuristic in `src/qpair.rs`: verify shallow queue selected for small batches, deep queue for large batches
- [X] T031 [P] [US3] Unit tests for batch dispatch in `src/actor.rs`: submit `Command::BatchSubmit` with N operations, verify N individual completions
- [X] T032 [P] [US3] Doc tests for `Command::BatchSubmit` in `src/command.rs`

### Implementation for User Story 3

- [X] T033 [US3] Implement batch handling in `src/actor.rs`: on `Command::BatchSubmit`, iterate ops, select queue pair via `QueuePairPool::select_qpair(batch_size)`, submit each op, track all as `PendingOp` entries
- [X] T034 [US3] Implement mixed-result handling for batches: valid operations succeed individually, invalid operations (e.g., out-of-range LBA) return individual `Completion::Error` entries
- [X] T035 [US3] Integration test in `tests/integration.rs`: submit batch of writes, verify all completions received

**Checkpoint**: User Stories 1, 2, AND 3 should all work independently.

---

## Phase 6: User Story 4 — NVMe Namespace Management (Priority: P2)

**Goal**: Clients probe, create, format, and delete NVMe namespaces via the messaging API.

**Independent Test**: Probe namespaces, create new one, verify it appears in subsequent probe, format it, delete it, verify removal.

### Tests for User Story 4

- [X] T036 [P] [US4] Unit tests for namespace probe in `src/namespace.rs`: verify probe returns list with correct properties
- [X] T037 [P] [US4] Unit tests for namespace create/format/delete lifecycle in `src/namespace.rs`
- [X] T038 [P] [US4] Doc tests for `Command::NsProbe`, `Command::NsCreate`, `Command::NsFormat`, `Command::NsDelete` in `src/command.rs`

### Implementation for User Story 4

- [X] T039 [US4] Wire namespace probe command in `src/actor.rs`: on `Command::NsProbe`, call `namespace::probe()`, send `Completion::NsProbeResult`
- [X] T040 [US4] Wire namespace create/format/delete commands in `src/actor.rs`: dispatch to `namespace::create()`/`format()`/`delete()`, send corresponding completions; serialize through actor (FR-020)
- [X] T041 [US4] Integration test in `tests/integration.rs`: full namespace lifecycle (probe → create → probe → format → delete → probe)

**Checkpoint**: User Stories 1–4 should all work independently.

---

## Phase 7: User Story 5 — Device Information and Telemetry (Priority: P3)

**Goal**: Clients query device capabilities via IBlockDevice and retrieve telemetry statistics when the `telemetry` feature is enabled.

**Independent Test**: Query device info, verify values match hardware. Run IO, query telemetry, verify stats populated. Without feature, verify error.

### Tests for User Story 5

- [X] T042 [P] [US5] Unit tests for device info methods in `src/lib.rs`: verify `sector_size`, `num_sectors`, `max_queue_depth`, `num_io_queues`, `max_transfer_size`, `block_size`, `numa_node`, `nvme_version` return correct values
- [X] T043 [P] [US5] Unit tests for telemetry in `src/telemetry.rs`: verify `TelemetryStats::record()` updates atomics correctly; verify `TelemetrySnapshot` computation
- [X] T044 [P] [US5] Unit test for telemetry API without feature: verify `IBlockDevice::telemetry()` returns `Err(BlockDeviceError::FeatureNotEnabled)`
- [X] T045 [P] [US5] Doc tests for `IBlockDevice` device info methods in `src/lib.rs` and `TelemetrySnapshot` in `src/telemetry.rs`

### Implementation for User Story 5

- [X] T046 [US5] Implement device info methods on `BlockDeviceSpdkNvmeComponent` in `src/lib.rs`: delegate to `NvmeController` fields populated at initialization
- [X] T047 [US5] Implement telemetry recording in `src/actor.rs`: on each completed IO operation (feature-gated), call `TelemetryStats::record(latency_ns, bytes)` to update atomic counters
- [X] T048 [US5] Implement `IBlockDevice::telemetry()` in `src/lib.rs`: when `telemetry` feature enabled, compute `TelemetrySnapshot` from `TelemetryStats`; when disabled, return `Err(BlockDeviceError::FeatureNotEnabled)`

**Checkpoint**: User Stories 1–5 should all work independently.

---

## Phase 8: User Story 6 — Controller Hardware Reset (Priority: P3)

**Goal**: Administrator issues hardware reset; controller reinitializes; pending operations are cancelled.

**Independent Test**: Issue reset, verify controller comes back online, perform read/write to confirm functionality.

### Tests for User Story 6

- [X] T049 [P] [US6] Unit tests for controller reset in `src/controller.rs`: verify `NvmeController::reset()` calls SPDK reset API and reinitializes queue pairs
- [X] T050 [P] [US6] Unit tests for pending op cancellation on reset in `src/actor.rs`: submit async ops, issue reset, verify all pending ops get `Completion::Error` callbacks
- [X] T051 [P] [US6] Doc tests for `Command::ControllerReset` in `src/command.rs`

### Implementation for User Story 6

- [X] T052 [US6] Implement `NvmeController::reset()` in `src/controller.rs`: call `spdk_nvme_ctrlr_reset`, reinitialize queue pairs, refresh namespace info
- [X] T053 [US6] Wire reset command in `src/actor.rs`: on `Command::ControllerReset`, cancel all pending ops across all clients with `Completion::Error`, call `controller.reset()`, send `Completion::ResetDone`
- [X] T054 [US6] Integration test in `tests/integration.rs`: reset controller, then perform read/write to verify functionality restored

**Checkpoint**: All user stories should now be independently functional.

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Benchmarks, formatting, documentation, and final validation

- [X] T055 [P] Create `benches/latency.rs` with Criterion benchmarks: sync read/write latency at queue depths 1, 4, 16, 64 for 4KB blocks using crossbeam bounded channel (64 slots)
- [X] T056 [P] Create `benches/throughput.rs` with Criterion benchmarks: batch write throughput at batch sizes 1, 8, 32, 128 for 4KB blocks using crossbeam bounded channel (64 slots)
- [X] T057 Run `cargo fmt --check` and fix any formatting violations across all source files
- [X] T058 Run `cargo clippy -- -D warnings` and fix any lint violations across all source files
- [X] T059 Run `cargo doc --no-deps` and fix any documentation warnings; verify all public APIs have doc comments with runnable examples
- [X] T060 Run `cargo test --all` and verify zero failures across unit, integration, and doc tests
- [X] T061 Run `cargo bench --no-run` and verify benchmarks compile without errors
- [X] T062 Run quickstart.md validation: follow the usage example end-to-end and verify it works

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Stories (Phases 3–8)**: All depend on Foundational phase completion
  - US1 and US2 (both P1) can proceed in parallel
  - US3 and US4 (both P2) can proceed in parallel after US1/US2 or independently
  - US5 and US6 (both P3) can proceed in parallel after earlier stories or independently
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational (Phase 2) — No dependencies on other stories
- **User Story 2 (P1)**: Can start after Foundational (Phase 2) — No dependencies on other stories (shares actor.rs with US1 but different code paths)
- **User Story 3 (P2)**: Can start after Foundational (Phase 2) — Independently testable; builds on qpair.rs selection heuristic
- **User Story 4 (P2)**: Can start after Foundational (Phase 2) — Independently testable; uses namespace.rs
- **User Story 5 (P3)**: Can start after Foundational (Phase 2) — Telemetry recording in actor depends on IO paths (US1/US2) being implemented, but device info is independent
- **User Story 6 (P3)**: Can start after Foundational (Phase 2) — Reset cancellation logic touches pending ops (US2), but can be implemented with stubs

### Within Each User Story

- Tests MUST be written and FAIL before implementation (TDD)
- Models/types before services/handlers
- Core implementation before integration tests
- Story complete before moving to next priority

### Parallel Opportunities

- All Setup tasks marked [P] (T003, T004, T005) can run in parallel
- All Foundational tasks marked [P] (T007, T008, T011) can run in parallel
- Once Foundational phase completes, user stories can start in parallel
- All tests within a story marked [P] can run in parallel
- All Polish benchmarks (T055, T056) can run in parallel

---

## Parallel Example: User Story 1

```bash
# Launch all tests for User Story 1 together:
Task: "T012 Unit tests for sync read/write dispatch in src/actor.rs"
Task: "T013 Unit tests for write-zeros dispatch in src/actor.rs"
Task: "T014 Unit test for out-of-range LBA read"
Task: "T015 Doc tests for ReadSync, WriteSync, WriteZeros in src/command.rs"

# Then implement sequentially:
Task: "T016 Implement sync read handling in src/actor.rs"
Task: "T017 Implement sync write handling in src/actor.rs"
Task: "T018 Implement write-zeros handling in src/actor.rs"
Task: "T019 Implement LBA bounds checking in src/actor.rs"
Task: "T020 Integration test: write-read roundtrip"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: User Story 1 (Basic Block IO)
4. **STOP and VALIDATE**: Test User Story 1 independently
5. Deploy/demo if ready

### Incremental Delivery

1. Complete Setup + Foundational → Foundation ready
2. Add User Story 1 (sync IO) → Test independently (MVP!)
3. Add User Story 2 (async IO) → Test independently
4. Add User Story 3 (batch ops) → Test independently
5. Add User Story 4 (namespace mgmt) → Test independently
6. Add User Story 5 (device info + telemetry) → Test independently
7. Add User Story 6 (controller reset) → Test independently
8. Polish phase: benchmarks, formatting, final validation

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together
2. Once Foundational is done:
   - Developer A: User Story 1 + User Story 2 (both P1, share actor.rs)
   - Developer B: User Story 3 + User Story 4 (both P2, different modules)
   - Developer C: User Story 5 + User Story 6 (both P3, different modules)
3. Stories complete and integrate independently

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Verify tests fail before implementing (TDD per Constitution Principle II)
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- Constitution quality gates must pass at every checkpoint:
  `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps`
