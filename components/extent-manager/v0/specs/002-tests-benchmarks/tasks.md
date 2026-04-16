# Tasks: Tests and Benchmarks

**Input**: Design documents from `specs/002-tests-benchmarks/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, quickstart.md

**Tests**: This feature IS tests — all tasks produce test code, mock infrastructure, or benchmarks.

**Organization**: Tasks grouped by user story (P1–P4) for independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1–US4)
- Include exact file paths in descriptions

---

## Phase 1: Setup

**Purpose**: Project structure for tests and benchmarks

- [X] T001 Create tests/ directory and benches/ directory at project root
- [X] T002 Uncomment [[bench]] entries in Cargo.toml and verify dev-dependencies include criterion 0.5

---

## Phase 2: Foundational (Mock Block Device Infrastructure)

**Purpose**: Test infrastructure that MUST be complete before ANY user story tests can run

**CRITICAL**: No test can run without the mock block device and heap DMA allocation

- [X] T003 Implement heap_dma_alloc() function in src/test_support.rs using DmaBuffer::from_raw() with std::alloc::alloc_zeroed for 4KiB-aligned heap memory
- [X] T004 Implement FaultConfig struct in src/test_support.rs with fail_after_n_writes (Option<u32>), fail_lba_range (Option<(u64, u64)>), and fail_all_writes (bool) fields
- [X] T005 Implement MockBlockDevice struct in src/test_support.rs: implements IBlockDevice with SpscChannel-based connect_client(), HashMap<u64, [u8; 4096]> block storage, and Arc<Mutex<FaultConfig>> for fault injection
- [X] T006 Implement MockBlockDevice actor thread in src/test_support.rs: background thread that receives Command::ReadSync/WriteSync, processes against in-memory blocks, checks FaultConfig before writes, sends Completion::ReadDone/WriteDone
- [X] T007 Add DmaAllocFn type alias and dma_alloc field to BlockDevice in src/block_device.rs; add BlockDevice::new_with_alloc() constructor; modify read_block() and write_block() to use (self.dma_alloc)() instead of DmaBuffer::new()
- [X] T008 Implement create_test_component() helper in src/test_support.rs: creates MockBlockDevice, wires to ExtentManagerComponentV1 receptacle, sets no-op flush_fn, calls initialize(), returns component and mock handle
- [X] T009 Add pub(crate) mod test_support declaration (behind #[cfg(test)]) in src/lib.rs

**Checkpoint**: Mock infrastructure ready — all user story tests can now use create_test_component()

---

## Phase 3: User Story 1 — API Operation Tests (Priority: P1)

**Goal**: Verify all IExtentManager operations work correctly through the full component stack

**Independent Test**: `cargo test -p extent-manager api_` passes with all API operation tests green

- [X] T010 [US1] Write test_create_and_lookup in tests/api_operations.rs: create extent, verify count, lookup metadata matches
- [X] T011 [P] [US1] Write test_create_and_remove in tests/api_operations.rs: create then remove, verify count returns to 0, lookup returns KeyNotFound
- [X] T012 [P] [US1] Write test_duplicate_key_error in tests/api_operations.rs: create same key twice, verify DuplicateKey error
- [X] T013 [P] [US1] Write test_key_not_found_error in tests/api_operations.rs: lookup and remove non-existent key, verify KeyNotFound error
- [X] T014 [P] [US1] Write test_invalid_size_class_error in tests/api_operations.rs: create with out-of-range size class, verify InvalidSizeClass error
- [X] T015 [P] [US1] Write test_out_of_space_error in tests/api_operations.rs: fill all slots in a size class, verify OutOfSpace error on next create
- [X] T016 [P] [US1] Write test_not_initialized_error in tests/api_operations.rs: call create_extent before initialize, verify NotInitialized error
- [X] T017 [P] [US1] Write test_device_too_small_error in tests/api_operations.rs: initialize with too-small device, verify IoError
- [X] T018 [US1] Write test_multiple_size_classes in tests/api_operations.rs: create extents across all configured size classes, verify each
- [X] T019 [US1] Write test_filename_and_crc in tests/api_operations.rs: create with filename and CRC, verify round-trip through lookup
- [X] T020 [US1] Write test_initialize_and_reopen in tests/api_operations.rs: initialize, create extents, re-open via open(), verify all extents recovered

**Checkpoint**: All IExtentManager operations tested — `cargo test -p extent-manager api_` green

---

## Phase 4: User Story 2 — Simulated Power-Failure Tests (Priority: P2)

**Goal**: Verify crash-consistency guarantees under simulated power-failure conditions

**Independent Test**: `cargo test -p extent-manager crash_` passes with all power-failure tests green

- [X] T021 [US2] Write test_orphan_after_record_write in tests/crash_recovery.rs: set FaultConfig to fail after 1 write (record succeeds, bitmap fails), verify orphan detected on re-open
- [X] T022 [US2] Write test_consistency_after_bitmap_fail_on_remove in tests/crash_recovery.rs: create extent, set fault on bitmap clear, attempt remove, re-open and verify extent still present
- [X] T023 [US2] Write test_recovery_after_clean_shutdown in tests/crash_recovery.rs: init, create multiple extents, re-open, verify all recovered with correct metadata
- [X] T024 [P] [US2] Write test_recovery_statistics in tests/crash_recovery.rs: verify RecoveryResult fields (extents_loaded, orphans_cleaned, corrupt_records) match expected values
- [X] T025 [P] [US2] Write test_corrupt_superblock_on_open in tests/crash_recovery.rs: write garbage to block 0, verify CorruptMetadata error on open

**Checkpoint**: Crash-consistency guarantees validated — `cargo test -p extent-manager crash_` green

---

## Phase 5: User Story 3 — Thread Safety Tests (Priority: P3)

**Goal**: Verify concurrent access correctness with no data corruption or deadlocks

**Independent Test**: `cargo test -p extent-manager thread_` passes with all thread-safety tests green

- [X] T026 [US3] Write test_concurrent_creates in tests/thread_safety.rs: 8 threads each creating 100 extents with unique key ranges, verify total count == 800 and each extent retrievable
- [X] T027 [US3] Write test_concurrent_creates_and_removes in tests/thread_safety.rs: threads doing mixed create/remove, verify final count == (creates - removes) and no phantom extents
- [X] T028 [US3] Write test_concurrent_lookups in tests/thread_safety.rs: create extents, then many threads doing concurrent lookups, verify all return correct metadata and complete within 30s timeout
- [X] T029 [US3] Write test_concurrent_mixed_operations in tests/thread_safety.rs: threads doing creates + removes + lookups simultaneously, verify consistent final state

**Checkpoint**: Thread safety validated — `cargo test -p extent-manager thread_` green

---

## Phase 6: User Story 4 — Performance Benchmarks (Priority: P4)

**Goal**: Establish performance baselines with Criterion benchmarks

**Independent Test**: `cargo bench -p extent-manager --no-run` compiles and `cargo bench -p extent-manager` produces results

- [X] T030 [P] [US4] Write create_extent benchmark in benches/create_benchmark.rs: measure throughput of create_extent with unique keys
- [X] T031 [P] [US4] Write lookup_extent benchmark in benches/lookup_benchmark.rs: pre-populate extents, measure lookup throughput
- [X] T032 [P] [US4] Write remove_extent benchmark in benches/remove_benchmark.rs: pre-populate, measure remove throughput
- [X] T033 [P] [US4] Write extent_count benchmark in benches/count_benchmark.rs: measure count with varying extent populations

**Checkpoint**: All benchmarks compile and produce results

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: CI gate compliance and cleanup

- [X] T034 Run cargo fmt -p extent-manager --check and fix any formatting issues
- [X] T035 Run cargo clippy -p extent-manager -- -D warnings and fix any lint issues
- [X] T036 Run cargo test -p extent-manager and verify all tests pass (existing 44 + new)
- [X] T037 Run cargo doc -p extent-manager --no-deps and verify zero warnings
- [X] T038 Run cargo bench -p extent-manager --no-run and verify benchmarks compile
- [X] T039 Update README.md with test execution instructions

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories
- **US1 API Tests (Phase 3)**: Depends on Foundational — can start after T009
- **US2 Power-Failure (Phase 4)**: Depends on Foundational — can start after T009, independent of US1
- **US3 Thread Safety (Phase 5)**: Depends on Foundational — can start after T009, independent of US1/US2
- **US4 Benchmarks (Phase 6)**: Depends on Foundational — can start after T009, independent of US1/US2/US3
- **Polish (Phase 7)**: Depends on all user stories complete

### User Story Dependencies

- **US1 (P1)**: After Foundational — no dependencies on other stories
- **US2 (P2)**: After Foundational — no dependencies on other stories
- **US3 (P3)**: After Foundational — no dependencies on other stories
- **US4 (P4)**: After Foundational — no dependencies on other stories

### Within Each User Story

- First test in each story sets up shared patterns (non-parallel)
- Subsequent tests marked [P] can run in parallel
- All tests in a story use the shared create_test_component() helper

### Parallel Opportunities

- T001, T002 can run in parallel (Phase 1)
- T003, T004 can run in parallel within Phase 2
- T011–T017 can run in parallel within US1 (all independent error path tests)
- T024, T025 can run in parallel within US2
- T030–T033 can run in parallel (all independent benchmark files)
- US1, US2, US3, US4 can all proceed in parallel once Phase 2 completes

---

## Parallel Example: User Story 1

```text
# After T010 establishes the pattern, launch all error-path tests together:
T011: "test_create_and_remove in tests/api_operations.rs"
T012: "test_duplicate_key_error in tests/api_operations.rs"
T013: "test_key_not_found_error in tests/api_operations.rs"
T014: "test_invalid_size_class_error in tests/api_operations.rs"
T015: "test_out_of_space_error in tests/api_operations.rs"
T016: "test_not_initialized_error in tests/api_operations.rs"
T017: "test_device_too_small_error in tests/api_operations.rs"
```

---

## Implementation Strategy

### MVP First (US1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (mock infrastructure)
3. Complete Phase 3: US1 API tests
4. **STOP and VALIDATE**: `cargo test -p extent-manager api_` passes
5. Constitution compliance: all public API operations have tests

### Incremental Delivery

1. Setup + Foundational → Mock infrastructure ready
2. Add US1 → All API operations tested (MVP!)
3. Add US2 → Crash consistency validated
4. Add US3 → Thread safety validated
5. Add US4 → Benchmarks established
6. Polish → Full CI gate compliance

---

## Notes

- All tests use the mock block device — no SPDK/hardware required
- test_support.rs is behind #[cfg(test)] — not compiled in production
- Benchmarks use mock block device (measuring algorithmic overhead, not I/O)
- Thread-safety tests use 30-second timeout for deadlock detection
- FaultConfig allows per-test control of which writes fail
