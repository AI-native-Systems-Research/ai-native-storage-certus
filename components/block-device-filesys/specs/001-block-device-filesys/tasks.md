# Tasks: Block Device Filesys Component

**Input**: Design documents from `specs/001-block-device-filesys/`

**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Included — explicitly requested in feature specification ("Create unit tests for the component and benchmarks").

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup

**Purpose**: Project initialization and crate structure

- [X] T001 Create Cargo.toml with dependencies: component-core, component-macros, component-framework, interfaces, io-uring, crossbeam-channel; dev-dependencies: criterion, tempfile in Cargo.toml
- [X] T002 [P] Create src/lib.rs with module declarations, re-exports, and define_component! macro for BlockDeviceFilesysComponent with provides: [IBlockDevice], receptacles: { logger: ILogger }
- [X] T003 [P] Create src/config.rs with DeviceConfig struct (file_path, block_size, num_blocks, total_bytes) and validation logic
- [X] T004 [P] Create src/telemetry.rs with feature-gated telemetry stub (mirrors block-device-spdk-nvme pattern: FeatureNotEnabled when not compiled)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core actor infrastructure that MUST be complete before user story IO operations

**CRITICAL**: No user story work can begin until this phase is complete

- [X] T005 Implement component configuration methods in src/lib.rs: set_file_path(), set_block_size(), set_num_blocks() operating on component fields
- [X] T006 Implement initialize() in src/lib.rs: validate config, create/open backing file via fallocate (create if absent, error on size mismatch), start actor thread
- [X] T007 Implement shutdown() in src/lib.rs: send Shutdown control message to actor, join actor thread, close file descriptor
- [X] T008 Create src/actor.rs with FilesysActor struct implementing ActorHandler<ControlMessage>: io_uring ring creation, client HashMap, inflight map, event loop skeleton
- [X] T009 Implement ControlMessage handling in src/actor.rs: ConnectClient (register session), DisconnectClient (remove session), Shutdown (break event loop)
- [X] T010 Implement client channel polling in src/actor.rs: drain all connected client ingress_rx channels for Command messages each iteration

**Checkpoint**: Actor starts, accepts client connections, and polls for commands

---

## Phase 3: User Story 1 — Block IO via File-Backed Device (Priority: P1)

**Goal**: Core read/write IO operations with file-backed storage and fdatasync durability

**Independent Test**: Create temp file, init component, write data, read back, verify match

### Tests for User Story 1

- [X] T011 [P] [US1] Unit test in src/lib.rs: verify component creation via define_component!, provides IBlockDevice, has logger receptacle
- [X] T012 [P] [US1] Unit test in src/config.rs: DeviceConfig validation — valid configs succeed, invalid (zero blocks, non-power-of-2 block_size) produce errors
- [X] T013 [P] [US1] Integration test in tests/integration.rs: initialize with temp file, verify backing file created with correct size via fallocate
- [X] T014 [US1] Integration test in tests/integration.rs: connect_client, send WriteSync + ReadSync at same LBA, verify data matches
- [X] T015 [US1] Integration test in tests/integration.rs: send WriteAsync + ReadAsync, verify completions arrive with correct OpHandles
- [X] T016 [P] [US1] Integration test in tests/integration.rs: send WriteZeros, read back zeroed LBA range, verify all zeros
- [X] T017 [P] [US1] Integration test in tests/integration.rs: read/write at LBA beyond device size, verify LbaOutOfRange error

### Implementation for User Story 1

- [X] T018 [US1] Implement ReadSync command processing in src/actor.rs: validate ns_id and LBA bounds, pread from backing file into DmaBuffer via as_mut_slice(), send ReadDone completion
- [X] T019 [US1] Implement WriteSync command processing in src/actor.rs: validate ns_id and LBA bounds, pwrite from DmaBuffer via as_slice(), fdatasync, send WriteDone completion
- [X] T020 [US1] Implement ReadAsync command processing in src/actor.rs: validate bounds, submit io_uring read SQE at offset lba×block_size, track in inflight map with OpHandle and deadline
- [X] T021 [US1] Implement WriteAsync command processing in src/actor.rs: validate bounds, submit io_uring write SQE linked (IOSQE_IO_LINK) to fsync SQE, track in inflight map
- [X] T022 [US1] Implement io_uring completion harvesting in src/actor.rs: iterate CQ entries, match user_data to inflight ops, send ReadDone/WriteDone to appropriate client
- [X] T023 [US1] Implement timeout handling in src/actor.rs: check deadline map each iteration, send Completion::Timeout for expired ops, submit AsyncCancel
- [X] T024 [US1] Implement WriteZeros command processing in src/actor.rs: allocate zero buffer, pwrite at offset, fdatasync, send WriteZerosDone completion
- [X] T025 [US1] Implement BatchSubmit processing in src/actor.rs: iterate ops vector, process each command sequentially, individual completions per op
- [X] T026 [US1] Implement AbortOp processing in src/actor.rs: submit io_uring AsyncCancel for target handle, send AbortAck completion
- [X] T027 [US1] Add doc tests for BlockDeviceFilesysComponent public items in src/lib.rs (component creation, configuration methods)
- [X] T028 [US1] Add doc tests for DeviceConfig in src/config.rs (construction and validation examples)

**Checkpoint**: Full read/write IO works with durability guarantees. User Story 1 independently testable.

---

## Phase 4: User Story 2 — Drop-In Replacement for SPDK Block Device (Priority: P2)

**Goal**: API-compatible IBlockDevice implementation with correct device introspection

**Independent Test**: Exercise all IBlockDevice query methods, verify values match config; bind via component-framework, query via IUnknown

### Tests for User Story 2

- [X] T029 [P] [US2] Integration test in tests/integration.rs: verify sector_size(), num_sectors(), block_size() return configured values
- [X] T030 [P] [US2] Integration test in tests/integration.rs: verify max_queue_depth(), num_io_queues(), max_transfer_size() return reasonable values
- [X] T031 [P] [US2] Integration test in tests/integration.rs: verify numa_node() returns -1, nvme_version() returns "N/A (file-backed)"
- [X] T032 [US2] Integration test in tests/integration.rs: verify NsProbe returns single NamespaceInfo with ns_id=1 and correct geometry
- [X] T033 [P] [US2] Integration test in tests/integration.rs: verify NsCreate, NsDelete, NsFormat, ControllerReset return NotSupported
- [X] T034 [US2] Integration test in tests/integration.rs: verify component provides IBlockDevice via IUnknown query

### Implementation for User Story 2

- [X] T035 [US2] Implement IBlockDevice trait methods in src/lib.rs: sector_size(), num_sectors(), block_size() returning configured values from component fields
- [X] T036 [US2] Implement IBlockDevice trait methods in src/lib.rs: max_queue_depth() (io_uring SQ size), num_io_queues() (1), max_transfer_size() (block_size×256)
- [X] T037 [US2] Implement IBlockDevice trait methods in src/lib.rs: numa_node() (-1), nvme_version() ("N/A (file-backed)")
- [X] T038 [US2] Implement connect_client() in src/lib.rs: create per-client SPSC channels (capacity 64), send ConnectClient to actor, return ClientChannels
- [X] T039 [US2] Implement NsProbe handling in src/actor.rs: return NsProbeResult with single NamespaceInfo { ns_id: 1, num_sectors, sector_size }
- [X] T040 [US2] Implement NotSupported responses in src/actor.rs: NsCreate, NsDelete, NsFormat, ControllerReset return Error completion with NotSupported
- [X] T041 [US2] Implement telemetry() in src/lib.rs: with feature gate matching block-device-spdk-nvme pattern
- [X] T042 [US2] Add doc tests for IBlockDevice methods in src/lib.rs (connect_client, sector_size, telemetry)

**Checkpoint**: Component is a verified drop-in replacement for IBlockDevice consumers. User Story 2 independently testable.

---

## Phase 5: User Story 3 — Performance Benchmarking (Priority: P3)

**Goal**: Criterion benchmarks measuring latency and throughput analogous to block-device-spdk-nvme

**Independent Test**: `cargo bench` produces benchmark results for latency and throughput groups

### Tests for User Story 3

- [X] T043 [US3] Verify `cargo bench` compiles and runs without errors (CI validation)

### Implementation for User Story 3

- [X] T044 [P] [US3] Create benches/latency.rs with Criterion benchmark: command_construction_latency measuring Command::WriteZeros creation at varying queue depths (1, 4, 16, 64)
- [X] T045 [P] [US3] Create benches/throughput.rs with Criterion benchmark: batch_construction_throughput measuring BatchSubmit construction at varying batch sizes (1, 8, 32, 128)
- [X] T046 [US3] Implement sync_io_latency benchmark in benches/latency.rs: measure actual pread/pwrite+fdatasync latency for 4KB blocks with temp backing file
- [ ] T047 [US3] Implement async_io_latency benchmark in benches/latency.rs: measure io_uring read/write latency for 4KB blocks at varying queue depths
- [X] T048 [US3] Implement write_throughput benchmark in benches/throughput.rs: measure sequential write throughput at varying block counts (1, 8, 32, 128 × 4KB)
- [X] T049 [US3] Add [[bench]] entries in Cargo.toml: name="latency" harness=false, name="throughput" harness=false

**Checkpoint**: Criterion benchmarks produce stable measurements. User Story 3 independently testable.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Final quality, documentation, and compliance

- [X] T050 [P] Run cargo fmt --check and fix any formatting issues across all source files
- [X] T051 [P] Run cargo clippy -- -D warnings and fix all warnings across all source files
- [X] T052 [P] Run cargo doc --no-deps and fix any documentation warnings
- [X] T053 Verify all doc tests pass with cargo test --doc
- [X] T054 Run full test suite: cargo test --all -- --test-threads 1 and verify zero failures
- [X] T055 Run cargo bench and verify all benchmarks execute without errors
- [X] T056 [P] Add edge case tests in tests/integration.rs: invalid file path initialization error, existing file with wrong size error, multiple client connections
- [ ] T057 Validate quickstart.md example compiles and runs (create a small integration test matching the example)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 (T001-T004) completion — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Phase 2 (T005-T010) completion
- **User Story 2 (Phase 4)**: Depends on Phase 2 completion. Can run in parallel with US1 for introspection methods; connect_client depends on T009
- **User Story 3 (Phase 5)**: Depends on Phase 3 completion (benchmarks need working IO)
- **Polish (Phase 6)**: Depends on all user stories complete

### Within Each User Story

- Tests written first (fail initially)
- Models/config before services/actors
- Core implementation before edge cases
- Integration tests validate each checkpoint

### Parallel Opportunities

Phase 1:
```
T002 (lib.rs) | T003 (config.rs) | T004 (telemetry.rs)  — all [P], different files
```

Phase 3 Tests:
```
T011 (unit/lib) | T012 (unit/config) | T013 (init) | T016 (zeros) | T017 (bounds)  — all [P]
```

Phase 4 Tests:
```
T029 (sector/block) | T030 (queue/transfer) | T031 (numa/version) | T033 (not-supported)  — all [P]
```

Phase 5 Implementation:
```
T044 (latency.rs) | T045 (throughput.rs)  — [P], different files
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001-T004)
2. Complete Phase 2: Foundational (T005-T010)
3. Complete Phase 3: User Story 1 (T011-T028)
4. **STOP and VALIDATE**: Run tests, verify read-after-write correctness
5. Component has working file-backed block IO with durability

### Incremental Delivery

1. Setup + Foundational → Actor infrastructure ready
2. Add User Story 1 → Core IO works → Test independently (MVP!)
3. Add User Story 2 → Full IBlockDevice API → Drop-in compatible
4. Add User Story 3 → Benchmarks → Performance baselined
5. Polish → Lint, docs, edge cases → Ship-ready

### Parallel Team Strategy

With multiple developers:
1. Team completes Setup + Foundational together
2. Once Foundational is done:
   - Developer A: User Story 1 (core IO)
   - Developer B: User Story 2 introspection methods (T035-T037, T041-T042)
3. After US1 done: Developer A moves to US3 (benchmarks need IO)
4. Polish in parallel

---

## Notes

- [P] tasks = different files, no dependencies
- [US*] label maps task to specific user story for traceability
- Each user story is independently completable and testable
- Tests are included as explicitly requested in the specification
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
