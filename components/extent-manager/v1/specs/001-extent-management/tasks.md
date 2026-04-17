# Tasks: Extent Management

**Input**: Design documents from `/specs/001-extent-management/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/interfaces.md

**Tests**: Included — the feature specification and constitution both require unit tests, integration tests, crash recovery tests, thread safety tests, doc tests, and benchmarks.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US7)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create project skeleton with all dependencies and module scaffolding

- [ ] T001 Create Cargo.toml with dependencies (component-core, component-macros, interfaces, crc32fast), dev-dependencies (criterion), features (default=["spdk"], testing), and bench harness config
- [ ] T002 Create module scaffolding in src/lib.rs with mod declarations for bitmap, block_device, error, metadata, recovery, superblock, state, test_support; create empty module files for each

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and test infrastructure that MUST be complete before ANY user story can be implemented

**CRITICAL**: No user story work can begin until this phase is complete

- [ ] T003 [P] Implement error types in src/error.rs — local error enum mapping to ExtentManagerError variants (CorruptMetadata, DuplicateKey, InvalidSizeClass, IoError, KeyNotFound, NotInitialized, OutOfSpace)
- [ ] T004 [P] Implement ExtentMetadata and OnDiskExtentRecord in src/metadata.rs — struct definitions, serialize/deserialize to 4KiB block, CRC-32 at bytes 4092-4096, filename length-prefixed UTF-8 (max 255 bytes), BLOCK_SIZE=4096 constant
- [ ] T005 [P] Implement BlockDevice wrapper in src/block_device.rs — typed read_block/write_block through IBlockDevice channel pair, DmaAllocFn type alias
- [ ] T006 Implement test support in src/test_support.rs — MockBlockDevice (HashMap<u64, [u8; 4096]> with actor thread), FaultConfig (fail_after_n_writes, fail_lba_range, fail_all_writes), heap_dma_alloc() (4KiB-aligned heap allocation), create_test_component() helper; gate with cfg(any(test, feature="testing"))
- [ ] T007 Implement ExtentManagerState in src/state.rs — in-memory state holding HashMap<u64, ExtentMetadata> index, Vec of AllocationBitmap per size class, size class config (sizes, slots, LBA offsets), namespace_id

**Checkpoint**: Foundation ready — user story implementation can now begin

---

## Phase 3: User Story 7 — Initialization with Multiple Size Classes (Priority: P1) MVP

**Goal**: Initialize the extent manager with configurable size classes, format the block device with superblock and empty bitmaps/records, and support reopening an existing volume

**Independent Test**: Initialize with various size/slot configurations, verify system accepts creates for each configured size and rejects unsupported sizes. Close and reopen, verify existing metadata loads correctly.

### Implementation for User Story 7

- [ ] T008 [US7] Implement Superblock in src/superblock.rs — magic 0x4558544D475256_31, format version 1, size/slot arrays, namespace_id, CRC-32 validation, serialize/deserialize to/from 4KiB block at LBA 0
- [ ] T009 [US7] Implement AllocationBitmap in src/bitmap.rs — new(num_slots), set/clear/is_set/find_free operations, serialize/deserialize to/from 4KiB blocks, word-scanning find_free with u64 bitmasks
- [ ] T010 [US7] Implement on-disk layout calculator in src/state.rs — compute bitmap_start_lba and record_start_lba for each size class given superblock config; bitmap blocks = ceil(slots / 32768) per class
- [ ] T011 [US7] Implement define_component! wiring in src/lib.rs — ExtentManagerComponentV1 with provides [IExtentManager, IExtentManagerAdmin], receptacles {block_device: IBlockDevice, logger: ILogger}, fields {state: RwLock<Option<ExtentManagerState>>, dma_alloc: Mutex<Option<DmaAllocFn>>}
- [ ] T012 [US7] Implement IExtentManagerAdmin::set_dma_alloc in src/lib.rs
- [ ] T013 [US7] Implement IExtentManagerAdmin::initialize in src/lib.rs — validate params (1-32 sizes, 128KiB-5MiB multiples of 4KiB, 1-10M slots), write superblock, zero all bitmap blocks, zero all record blocks, build initial ExtentManagerState
- [ ] T014 [US7] Implement IExtentManagerAdmin::open in src/lib.rs — read superblock, validate magic/version/CRC, load all bitmap blocks into memory, build in-memory index by scanning record blocks (defer recovery to US5 phase, for now just load valid records)

### Tests for User Story 7

- [ ] T015 [P] [US7] Unit tests for Superblock serialize/deserialize and CRC validation in src/superblock.rs
- [ ] T016 [P] [US7] Unit tests for AllocationBitmap set/clear/is_set/find_free operations in src/bitmap.rs
- [ ] T017 [US7] Integration tests in tests/api_operations.rs — initialize with 1/3/32 size classes, verify superblock persisted, initialize with invalid params (0 sizes, size not multiple of 4KiB, >32 classes), open after initialize and verify state, open uninitialized device returns error

**Checkpoint**: Extent manager can be initialized and reopened. MVP foundation complete.

---

## Phase 4: User Story 1 — Create and Allocate Extents (Priority: P1)

**Goal**: Allocate new fixed-size extents on disk with two-phase atomic writes, persist metadata, return extent location

**Independent Test**: Create an extent with key and size, verify it exists with correct metadata. Create with optional filename/CRC, verify fields stored. Test duplicate key and out-of-space errors.

### Implementation for User Story 1

- [ ] T018 [US1] Implement IExtentManager::create_extent in src/lib.rs — acquire write lock, validate key uniqueness and size class, find free slot via bitmap, two-phase write (write record block at record_start_lba + slot_index, then set bitmap bit and write bitmap block), update in-memory index, return serialized metadata
- [ ] T019 [US1] Implement extent metadata serialization for return value in src/metadata.rs — serialize ExtentMetadata to Vec<u8> for create_extent and lookup_extent return

### Tests for User Story 1

- [ ] T020 [P] [US1] Unit tests for OnDiskExtentRecord serialize/deserialize with and without filename/CRC in src/metadata.rs
- [ ] T021 [US1] Integration tests in tests/api_operations.rs — create extent and verify metadata persisted, create with filename and CRC and verify fields, create with duplicate key returns DuplicateKey error, create with unsupported size returns InvalidSizeClass, exhaust all slots and verify OutOfSpace error

**Checkpoint**: Extents can be created and persisted to disk.

---

## Phase 5: User Story 2 — Look Up and Read Extent Metadata (Priority: P1)

**Goal**: Look up extent metadata by key, returning on-disk location and all stored fields

**Independent Test**: Create an extent, look it up by key, verify all metadata fields match. Look up nonexistent key, verify KeyNotFound error.

### Implementation for User Story 2

- [ ] T022 [US2] Implement IExtentManager::lookup_extent in src/lib.rs — acquire read lock, look up key in HashMap index, return serialized metadata or KeyNotFound
- [ ] T023 [US2] Implement IExtentManager::extent_count in src/lib.rs — acquire read lock, return index length

### Tests for User Story 2

- [ ] T024 [US2] Integration tests in tests/api_operations.rs — create then lookup by key and verify all fields (namespace_id, offset, size, filename, CRC), lookup nonexistent key returns KeyNotFound, extent_count returns correct count after creates

**Checkpoint**: Full create-and-lookup cycle works. Core read path complete.

---

## Phase 6: User Story 3 — Remove Extents and Free Space (Priority: P2)

**Goal**: Remove extents by key, free allocation slots for reuse

**Independent Test**: Create extent, remove by key, verify lookup returns KeyNotFound, create new extent of same size and verify freed space reused.

### Implementation for User Story 3

- [ ] T025 [US3] Implement IExtentManager::remove_extent in src/lib.rs — acquire write lock, look up key in index, two-phase write (clear bitmap bit and write bitmap block, then zero record block), remove from in-memory index

### Tests for User Story 3

- [ ] T026 [US3] Integration tests in tests/api_operations.rs — create then remove then verify lookup returns KeyNotFound, remove nonexistent key returns KeyNotFound, create-remove-create cycle verifies slot reuse, extent_count decrements after removal

**Checkpoint**: Full CRUD lifecycle works (create, lookup, remove).

---

## Phase 7: User Story 4 — Iterate All Extents (Priority: P2)

**Goal**: Iterate through all stored extents visiting each exactly once, with exclusive lock blocking concurrent modifications

**Independent Test**: Create multiple extents across size classes, iterate all, verify each visited exactly once. Iterate empty manager, verify zero results.

### Implementation for User Story 4

- [ ] T027 [US4] Implement extent iteration in src/lib.rs — acquire read lock (blocks write lock holders = create/remove), iterate HashMap index, invoke caller-provided callback or return iterator over all ExtentMetadata entries

### Tests for User Story 4

- [ ] T028 [US4] Integration tests in tests/api_operations.rs — create N extents across multiple size classes, iterate and verify each key seen exactly once, iterate empty manager returns zero results, verify count matches extent_count

**Checkpoint**: All four core operations work (create, lookup, remove, iterate).

---

## Phase 8: User Story 5 — Crash Recovery (Priority: P1)

**Goal**: Recover to consistent state after power failure during any operation, with no data loss, space leaks, or corrupt metadata

**Independent Test**: Simulate power failure at various points during create/remove, restart manager, verify consistent state.

### Implementation for User Story 5

- [ ] T029 [US5] Implement recovery logic in src/recovery.rs — scan each size class: for each slot, read record block and check CRC, compare against bitmap bit, apply recovery matrix (bitmap=1+valid CRC → keep, bitmap=1+invalid CRC → clear bitmap + zero record, bitmap=0+valid CRC → zero record orphan, bitmap=0+invalid → skip), return RecoveryResult with counts
- [ ] T030 [US5] Integrate recovery into IExtentManagerAdmin::open in src/lib.rs — call recovery after loading bitmaps, rebuild in-memory index from surviving records

### Tests for User Story 5

- [ ] T031 [P] [US5] Crash recovery test in tests/crash_recovery.rs — power failure during create after record written but before bitmap updated (FaultConfig fail_after_n_writes), reboot, verify orphan cleaned and space available
- [ ] T032 [P] [US5] Crash recovery test in tests/crash_recovery.rs — power failure during create after bitmap updated, reboot, verify extent intact and accessible
- [ ] T033 [P] [US5] Crash recovery test in tests/crash_recovery.rs — power failure during remove, reboot, verify consistent state with no space leaks
- [ ] T034 [US5] Crash recovery test in tests/crash_recovery.rs — corrupt CRC record detected on open, bitmap cleared, space reclaimed
- [ ] T035 [US5] Crash recovery test in tests/crash_recovery.rs — multiple crash-restart cycles, verify no progressive degradation

**Checkpoint**: System recovers correctly from any crash scenario. All P1 stories complete.

---

## Phase 9: User Story 6 — Concurrent Access (Priority: P2)

**Goal**: Thread-safe access to all operations with no data races, corruption, or deadlocks

**Independent Test**: Run multiple threads performing mixed operations simultaneously, verify all complete correctly.

### Tests for User Story 6

- [ ] T036 [P] [US6] Thread safety test in tests/thread_safety.rs — 8+ threads concurrently creating extents, verify all allocated with unique space, no overlaps
- [ ] T037 [P] [US6] Thread safety test in tests/thread_safety.rs — 8+ threads performing mixed create/remove/lookup, verify consistent state after all complete, no space leaks
- [ ] T038 [US6] Thread safety test in tests/thread_safety.rs — verify iteration blocks concurrent create/remove operations (attempt create during iteration, verify it waits)

**Checkpoint**: All user stories complete and independently verified.

---

## Phase 10: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, benchmarks, and CI gate compliance

- [ ] T039 [P] Create Criterion benchmark for create operations (single + bulk) in benches/create_benchmark.rs
- [ ] T040 [P] Create Criterion benchmark for remove operations (single + bulk) in benches/remove_benchmark.rs
- [ ] T041 [P] Create Criterion benchmark for lookup operations in benches/lookup_benchmark.rs
- [ ] T042 [P] Create Criterion benchmark for iteration in benches/iterate_benchmark.rs
- [ ] T043 Add rustdoc comments with runnable examples to all public types, functions, and methods across src/
- [ ] T044 Create README.md with component description, build/test/bench instructions, usage example
- [ ] T045 Run full CI gate: cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo doc --no-deps && cargo bench --no-run
- [ ] T046 Run quickstart.md validation — verify all commands from quickstart.md work correctly

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US7 Initialization (Phase 3)**: Depends on Foundational — BLOCKS all other user stories
- **US1 Create (Phase 4)**: Depends on US7
- **US2 Lookup (Phase 5)**: Depends on US1 (needs extents to look up)
- **US3 Remove (Phase 6)**: Depends on US1 (needs extents to remove)
- **US4 Iterate (Phase 7)**: Depends on US1 (needs extents to iterate)
- **US5 Crash Recovery (Phase 8)**: Depends on US1 + US3 (needs create/remove to test recovery)
- **US6 Concurrent Access (Phase 9)**: Depends on US1 + US3 (needs all mutation operations)
- **Polish (Phase 10)**: Depends on all user stories being complete

### User Story Dependencies

- **US7 (Init)**: First — everything depends on initialization
- **US1 (Create)**: After US7 — other stories need extents to exist
- **US2 (Lookup)**: After US1 — can proceed in parallel with US3, US4
- **US3 (Remove)**: After US1 — can proceed in parallel with US2, US4
- **US4 (Iterate)**: After US1 — can proceed in parallel with US2, US3
- **US5 (Crash Recovery)**: After US1 + US3 — needs both write paths
- **US6 (Concurrent)**: After US1 + US3 — needs all mutation operations

### Within Each User Story

- Implementation tasks before test tasks (where tests depend on implementation)
- Core logic before integration
- Story complete before moving to next priority

### Parallel Opportunities

- T003, T004, T005 can run in parallel (independent foundational modules)
- T015, T016 can run in parallel (independent unit test files)
- T020 can run in parallel with T018-T019 (test file vs impl file)
- US2, US3, US4 can run in parallel after US1 completes
- T031, T032, T033 can run in parallel (independent crash scenarios in same file)
- T036, T037 can run in parallel (independent thread tests)
- T039, T040, T041, T042 can run in parallel (independent benchmark files)

---

## Parallel Example: Foundational Phase

```
# These three modules are independent and can be written simultaneously:
Task T003: "Implement error types in src/error.rs"
Task T004: "Implement metadata types in src/metadata.rs"
Task T005: "Implement block device wrapper in src/block_device.rs"
```

## Parallel Example: After US1 Completes

```
# These three stories can proceed in parallel:
Phase 5 (US2 Lookup): T022-T024
Phase 6 (US3 Remove): T025-T026
Phase 7 (US4 Iterate): T027-T028
```

## Parallel Example: Benchmarks

```
# All four benchmark files are independent:
Task T039: "Create benchmark in benches/create_benchmark.rs"
Task T040: "Create benchmark in benches/remove_benchmark.rs"
Task T041: "Create benchmark in benches/lookup_benchmark.rs"
Task T042: "Create benchmark in benches/iterate_benchmark.rs"
```

---

## Implementation Strategy

### MVP First (US7 + US1 + US2)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: US7 Initialization
4. Complete Phase 4: US1 Create Extents
5. Complete Phase 5: US2 Lookup Extents
6. **STOP and VALIDATE**: Can initialize, create, and look up extents
7. This is a functional MVP — data can be stored and retrieved

### Incremental Delivery

1. Setup + Foundational + US7 → System can initialize
2. Add US1 (Create) + US2 (Lookup) → MVP: store and retrieve extents
3. Add US3 (Remove) → Full CRUD lifecycle
4. Add US4 (Iterate) → Index rebuild capability
5. Add US5 (Crash Recovery) → Production-grade reliability
6. Add US6 (Concurrent Access) → Production-grade threading
7. Polish → Benchmarks, docs, CI gate compliance

### Sequential Execution (Single Developer)

Phases 1→2→3→4→5→6→7→8→9→10 in order. Each phase builds on the previous. Stop at any checkpoint to validate.

---

## Notes

- [P] tasks = different files, no dependencies on incomplete tasks
- [Story] label maps task to specific user story for traceability
- Each user story is independently testable at its checkpoint
- Commit after each task or logical group
- The spec requires tests — all test tasks are mandatory per constitution Principles 1 and 2
