# Tasks: Dispatcher Cache Interface

**Input**: Design documents from `/specs/001-dispatcher-cache-interface/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Included — constitution Principle III (Comprehensive Testing) is NON-NEGOTIABLE. Every public interface method requires unit tests, doc tests, and edge case coverage.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Define the IDispatcher interface, DispatcherError, and supporting types in the interfaces crate

- [x] T001 [P] Define `IDispatcher` interface, `DispatcherError`, `IpcHandle`, and `DispatcherConfig` types in `components/interfaces/src/idispatcher.rs`
- [x] T002 [P] Add `idispatcher` module and re-exports to `components/interfaces/src/lib.rs`
- [x] T003 Update `components/dispatcher/v0/Cargo.toml` with dev-dependencies for Criterion benchmarks

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core internal modules and component restructuring that MUST be complete before ANY user story

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 [P] Implement MDTS-aware I/O segmentation logic in `components/dispatcher/v0/src/io_segmenter.rs`
- [x] T005 [P] Implement background write worker thread skeleton (channel, shutdown flag, job struct) in `components/dispatcher/v0/src/background.rs`
- [x] T006 Restructure `define_component!` in `components/dispatcher/v0/src/lib.rs` — remove `block_device_admin` receptacle, add `fields` for internal state (background thread handle, device storage, initialized flag)
- [x] T007 [P] Unit tests for I/O segmenter — verify correct splitting at MDTS boundary, single-segment passthrough, zero-size edge case in `components/dispatcher/v0/src/io_segmenter.rs`
- [x] T008 [P] Unit tests for background worker — verify clean startup and shutdown, job channel communication in `components/dispatcher/v0/src/background.rs`

**Checkpoint**: Foundation ready — user story implementation can now begin

---

## Phase 3: User Story 5 — Dispatcher Initialization and Wiring (Priority: P1)

**Goal**: System integrator can wire the dispatcher to its dependencies and initialize it. All N block devices and N extent managers are created and configured.

**Independent Test**: Wire all receptacles, call initialize with a DispatcherConfig, verify the dispatcher transitions to operational state. Call initialize without receptacles and verify descriptive error.

### Tests for User Story 5

- [x] T009 [P] [US5] Unit test: initialize without receptacles returns `DispatcherError::NotInitialized` in `components/dispatcher/v0/src/lib.rs`
- [x] T010 [P] [US5] Unit test: initialize with empty `data_pci_addrs` returns `DispatcherError::InvalidParameter` in `components/dispatcher/v0/src/lib.rs`
- [x] T011 [P] [US5] Doc test for `IDispatcher::initialize()` in `components/interfaces/src/idispatcher.rs`
- [x] T012 [P] [US5] Doc test for `IDispatcher::shutdown()` in `components/interfaces/src/idispatcher.rs`

### Implementation for User Story 5

- [x] T013 [US5] Implement `IDispatcher::initialize()` — validate receptacles, create N block device instances (set PCI address, initialize), create N extent managers (wire to metadata namespace, format with data disk size and instance ID), start background write worker in `components/dispatcher/v0/src/lib.rs`
- [x] T014 [US5] Implement `IDispatcher::shutdown()` — signal background writer to stop, join thread, shutdown all block devices and extent managers in `components/dispatcher/v0/src/lib.rs`

**Checkpoint**: Dispatcher can be created, wired, initialized, and shut down. Foundation for all cache operations.

---

## Phase 4: User Story 1 — Cache Population (Priority: P1) MVP

**Goal**: Client can populate a cache entry by DMA-copying GPU data into a staging buffer, with asynchronous background write to SSD.

**Independent Test**: Call populate with a new key, verify dispatch map contains the entry. Wait for background write, verify entry transitions to block-device state and staging buffer is freed.

### Tests for User Story 1

- [x] T015 [P] [US1] Unit test: populate with new key succeeds and entry appears in dispatch map in `components/dispatcher/v0/src/lib.rs`
- [x] T016 [P] [US1] Unit test: populate with duplicate key returns `DispatcherError::AlreadyExists` in `components/dispatcher/v0/src/lib.rs`
- [x] T017 [P] [US1] Unit test: populate with zero-size IpcHandle returns `DispatcherError::InvalidParameter` in `components/dispatcher/v0/src/lib.rs`
- [x] T018 [P] [US1] Doc test for `IDispatcher::populate()` in `components/interfaces/src/idispatcher.rs`

### Implementation for User Story 1

- [x] T019 [US1] Implement `IDispatcher::populate()` — take_write on dispatch map, create_staging, DMA copy from IPC handle, downgrade reference, enqueue background write job in `components/dispatcher/v0/src/lib.rs`
- [x] T020 [US1] Implement background write job processing — take_write, MDTS-segmented write to block device via BatchSubmit, convert_to_storage on success, free staging buffer in `components/dispatcher/v0/src/background.rs`
- [x] T021 [US1] Implement background write failure handling — on I/O error or out-of-space, remove entry from dispatch map, free staging buffer, log error in `components/dispatcher/v0/src/background.rs`
- [x] T022 [US1] Unit test: background write completion transitions entry from staging to block-device state in `components/dispatcher/v0/src/lib.rs`
- [x] T023 [US1] Unit test: background write failure removes entry and frees staging in `components/dispatcher/v0/src/lib.rs`

**Checkpoint**: Full populate path works — GPU data enters the cache and is persisted to SSD asynchronously.

---

## Phase 5: User Story 2 — Cache Lookup with DMA Transfer (Priority: P1)

**Goal**: Client can retrieve cached data by DMA-copying from staging buffer or SSD into GPU memory.

**Independent Test**: Populate an entry, look it up from staging state. Wait for SSD commit, look it up from block-device state. Verify correct data transfer in both cases.

### Tests for User Story 2

- [x] T024 [P] [US2] Unit test: lookup from staging state performs DMA copy to IPC handle in `components/dispatcher/v0/src/lib.rs`
- [x] T025 [P] [US2] Unit test: lookup from block-device state reads from SSD and DMA copies in `components/dispatcher/v0/src/lib.rs`
- [x] T026 [P] [US2] Unit test: lookup for non-existent key returns `DispatcherError::KeyNotFound` in `components/dispatcher/v0/src/lib.rs`
- [x] T027 [P] [US2] Doc test for `IDispatcher::lookup()` in `components/interfaces/src/idispatcher.rs`

### Implementation for User Story 2

- [x] T028 [US2] Implement `IDispatcher::lookup()` staging path — take_read, query dispatch map, DMA copy from staging buffer to IPC handle, release_read in `components/dispatcher/v0/src/lib.rs`
- [x] T029 [US2] Implement `IDispatcher::lookup()` SSD path — MDTS-segmented read from block device, DMA copy to IPC handle in `components/dispatcher/v0/src/lib.rs`

**Checkpoint**: Full round-trip works — populate then lookup returns the correct cached data.

---

## Phase 6: User Story 3 — Cache Presence Check (Priority: P2)

**Goal**: Client can check whether a cache entry exists without any data transfer.

**Independent Test**: Check a non-existent key (returns false), populate an entry, check again (returns true).

### Tests for User Story 3

- [x] T030 [P] [US3] Unit test: check for non-existent key returns false in `components/dispatcher/v0/src/lib.rs`
- [x] T031 [P] [US3] Unit test: check for existing key returns true in `components/dispatcher/v0/src/lib.rs`
- [x] T032 [P] [US3] Doc test for `IDispatcher::check()` in `components/interfaces/src/idispatcher.rs`

### Implementation for User Story 3

- [x] T033 [US3] Implement `IDispatcher::check()` — query dispatch map lookup, return bool based on result in `components/dispatcher/v0/src/lib.rs`

**Checkpoint**: Presence checks work independently of data transfer.

---

## Phase 7: User Story 4 — Cache Entry Removal (Priority: P2)

**Goal**: Client can evict a cache entry, freeing staging buffers and/or SSD extents.

**Independent Test**: Populate an entry, remove it, verify key is no longer present and resources are freed.

### Tests for User Story 4

- [x] T034 [P] [US4] Unit test: remove entry in staging state frees buffer and removes from dispatch map in `components/dispatcher/v0/src/lib.rs`
- [x] T035 [P] [US4] Unit test: remove entry in block-device state frees extent and removes from dispatch map in `components/dispatcher/v0/src/lib.rs`
- [x] T036 [P] [US4] Unit test: remove non-existent key returns `DispatcherError::KeyNotFound` in `components/dispatcher/v0/src/lib.rs`
- [x] T037 [P] [US4] Unit test: remove during in-flight background write blocks until write completes then removes in `components/dispatcher/v0/src/lib.rs`
- [x] T038 [P] [US4] Doc test for `IDispatcher::remove()` in `components/interfaces/src/idispatcher.rs`

### Implementation for User Story 4

- [x] T039 [US4] Implement `IDispatcher::remove()` — take_write (blocks if background write in progress), determine state, free staging or SSD extent, remove dispatch map entry in `components/dispatcher/v0/src/lib.rs`

**Checkpoint**: Full cache lifecycle works — populate, lookup, check, remove.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Concurrency tests, benchmarks, documentation, and quality gate validation

- [x] T040 [P] Concurrency test: concurrent populate and lookup on different keys — no deadlock or data corruption in `components/dispatcher/v0/src/lib.rs`
- [x] T041 [P] Concurrency test: concurrent lookups on the same key — multiple readers allowed in `components/dispatcher/v0/src/lib.rs`
- [x] T042 [P] Concurrency test: lookup during in-flight populate on same key — blocks until staging completes in `components/dispatcher/v0/src/lib.rs`
- [x] T043 [P] Criterion benchmarks for populate, lookup, check, remove in `components/dispatcher/v0/benches/dispatcher_benchmark.rs`
- [x] T044 [P] Module-level documentation for `lib.rs`, `io_segmenter.rs`, `background.rs` in `components/dispatcher/v0/src/`
- [x] T045 [P] Doc tests for `DispatcherError`, `DispatcherConfig`, `IpcHandle` in `components/interfaces/src/idispatcher.rs`
- [x] T046 Run full quality gate: `cargo fmt -p dispatcher --check && cargo clippy -p dispatcher -- -D warnings && cargo test -p dispatcher && cargo doc -p dispatcher --no-deps`
- [x] T047 Update `components/dispatcher/v0/README.md` with component overview, usage examples, and architecture

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup (T001, T002) — BLOCKS all user stories
- **US5 Initialization (Phase 3)**: Depends on Foundational — BLOCKS US1, US2, US3, US4
- **US1 Population (Phase 4)**: Depends on US5 — prerequisite for US2 testing
- **US2 Lookup (Phase 5)**: Depends on US1 (needs populated entries to look up)
- **US3 Check (Phase 6)**: Depends on US5 only — can run in PARALLEL with US1/US2
- **US4 Removal (Phase 7)**: Depends on US1 (needs populated entries to remove)
- **Polish (Phase 8)**: Depends on all user stories being complete

### User Story Dependencies

```
Phase 1 (Setup) → Phase 2 (Foundational)
                        ↓
                  Phase 3 (US5: Init)
                  ↙     ↓        ↘
    Phase 4 (US1)  Phase 6 (US3)  (US3 can run parallel)
         ↓
    Phase 5 (US2)
    Phase 7 (US4)
         ↓
    Phase 8 (Polish)
```

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Internal modules (io_segmenter, background) before interface methods
- Core path before error handling
- Story complete before moving to next priority

### Parallel Opportunities

- T001 + T002 can run in parallel (different files)
- T004 + T005 can run in parallel (different files)
- T007 + T008 can run in parallel (test files)
- All US5 tests (T009-T012) can run in parallel
- All US1 tests (T015-T018) can run in parallel
- US3 (Phase 6) can run in parallel with US1 (Phase 4) and US2 (Phase 5)
- All Polish tasks (T040-T045) can run in parallel

---

## Parallel Example: User Story 1

```bash
# Launch all tests for US1 together (before implementation):
Task: T015 "Unit test: populate with new key succeeds"
Task: T016 "Unit test: populate with duplicate key returns AlreadyExists"
Task: T017 "Unit test: populate with zero-size IpcHandle returns InvalidParameter"
Task: T018 "Doc test for populate()"

# Then implement sequentially:
Task: T019 "Implement populate() main path"
Task: T020 "Implement background write job processing"
Task: T021 "Implement background write failure handling"

# Then verify tests pass:
Task: T022 "Unit test: background write completion"
Task: T023 "Unit test: background write failure"
```

---

## Implementation Strategy

### MVP First (User Story 5 + User Story 1)

1. Complete Phase 1: Setup (interface definitions)
2. Complete Phase 2: Foundational (io_segmenter, background worker)
3. Complete Phase 3: US5 Initialization — dispatcher can be wired and started
4. Complete Phase 4: US1 Population — data enters the cache
5. **STOP and VALIDATE**: Test populate end-to-end, verify staging → SSD flow
6. This is the minimum viable dispatcher

### Incremental Delivery

1. Setup + Foundational → Interface and internal infrastructure ready
2. Add US5 (Init) → Dispatcher boots → Validate
3. Add US1 (Populate) → Data enters cache → Validate (MVP!)
4. Add US2 (Lookup) → Data can be retrieved → Validate (full round-trip!)
5. Add US3 (Check) → Lightweight presence queries → Validate
6. Add US4 (Remove) → Cache eviction → Validate (complete feature!)
7. Polish → Benchmarks, concurrency tests, docs

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Tests are REQUIRED by constitution Principle III — write tests first, verify they fail
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- MDTS segmentation (io_segmenter) is foundational — used by both populate (write) and lookup (read)
