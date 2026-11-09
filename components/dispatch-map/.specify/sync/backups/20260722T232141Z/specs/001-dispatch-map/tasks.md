# Tasks: Dispatch Map Component

**Input**: Design documents from `specs/001-dispatch-map/`
**Prerequisites**: plan.md, spec.md, data-model.md, contracts/idispatch_map.md, research.md, quickstart.md

**Tests**: Required by constitution (Principles III, IV, VI). All public APIs must have unit tests, doc tests, and integration tests.

**Organization**: Tasks grouped by user story. Reference counting (US4) is implemented first as it underpins all other stories.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story (US1–US6)
- Exact file paths included

---

## Phase 1: Setup

**Purpose**: Project initialization and file structure

- [x] T001 Update Cargo.toml to add criterion dev-dependency and any needed test dependencies in components/dispatch-map/v0/Cargo.toml
- [x] T002 Create source module files src/entry.rs and src/state.rs with module declarations in src/lib.rs

---

## Phase 2: Foundational (Interface Types & Core Data Structures)

**Purpose**: Define all shared types in the interfaces crate and core internal data structures. MUST complete before any user story.

- [x] T003 Define CacheKey type alias, DispatchMapError enum (with Display, Error impls), and LookupResult enum in components/interfaces/src/idispatch_map.rs
- [x] T004 Rewrite IDispatchMap trait with full method signatures (spdk-gated, matching contracts/idispatch_map.md) in components/interfaces/src/idispatch_map.rs
- [x] T005 Update components/interfaces/src/lib.rs with re-exports for CacheKey, DispatchMapError, LookupResult, and spdk-gated IDispatchMap
- [x] T006 [P] Implement Location enum and DispatchEntry struct with doc comments in src/entry.rs
- [x] T007 [P] Implement DispatchMapState struct (Mutex + Condvar + entries HashMap + buffers HashMap + dma_alloc Option) with blocking helper method in src/state.rs
- [x] T008 Update define_component! macro invocation and IDispatchMap impl skeleton (all methods returning todo!) in src/lib.rs

**Checkpoint**: All types compile; `cargo build -p dispatch-map` succeeds with todo! stubs.

---

## Phase 3: User Story 4 — Reference Counting for Concurrent Access (Priority: P1)

**Goal**: Implement readers-writer reference counting with configurable timeout blocking. This is the concurrency foundation for all other stories.

**Independent Test**: Acquire read refs from multiple threads, verify they succeed; attempt write ref from another thread, verify it blocks until reads are released; verify timeout returns error.

### Implementation

- [x] T009 [US4] Implement take_read(key, timeout) and take_write(key, timeout) with Condvar wait_timeout loop in src/lib.rs
- [x] T010 [P] [US4] Implement release_read(key) and release_write(key) with underflow error checking and condvar.notify_all() in src/lib.rs
- [x] T011 [US4] Implement downgrade_reference(key) — atomic write-to-read transition in src/lib.rs
- [x] T012 [US4] Add doc tests for take_read, take_write, release_read, release_write, downgrade_reference in src/lib.rs
- [x] T013 [US4] Unit tests: single-threaded ref counting — take/release happy path, underflow errors, no-write downgrade error in src/lib.rs (#[cfg(test)] module)
- [x] T014 [US4] Integration tests: multi-threaded concurrent access — multiple readers, writer blocks until readers release, timeout error after deadline in tests/integration.rs

**Checkpoint**: All ref counting methods work correctly under single-threaded and multi-threaded access. `cargo test -p dispatch-map` passes.

---

## Phase 4: User Story 1 — Staging Buffer Allocation (Priority: P1)

**Goal**: Allocate DMA staging buffers for incoming data via `create_staging`, with implicit write reference.

**Independent Test**: Call `create_staging(key, size)`, verify DMA buffer returned, entry exists with write_ref=1, duplicate calls rejected.

### Implementation

- [x] T015 [US1] Implement set_dma_alloc(alloc) — store DmaAllocFn in component state in src/lib.rs
- [x] T016 [US1] Implement create_staging(key, size) — validate size>0, allocate DMA buffer via DmaAllocFn, insert entry with write_ref=1, store buffer in side map, return ptr in src/lib.rs
- [x] T017 [US1] Add doc tests for set_dma_alloc and create_staging in src/lib.rs
- [x] T018 [US1] Unit tests: create_staging happy path, size=0 error, allocation failure error, duplicate key error in src/lib.rs (#[cfg(test)] module)

**Checkpoint**: Staging buffers can be allocated and entries tracked. Tests pass.

---

## Phase 5: User Story 2 — Looking Up Cached Data by Key (Priority: P1)

**Goal**: Look up extent data by key, returning location type with read reference acquisition and timeout blocking.

**Independent Test**: Stage data for a key, release write ref, call lookup, verify correct location type returned and read_ref incremented.

### Implementation

- [x] T019 [US2] Implement lookup(key, timeout) — check entry exists, block if write_ref>0 (reuse blocking helper), return LookupResult variant, increment read_ref in src/lib.rs
- [x] T020 [US2] Add doc tests for lookup in src/lib.rs
- [x] T021 [US2] Unit tests: lookup NotExist, Staging result, BlockDevice result, blocked-by-writer with timeout, MismatchSize in src/lib.rs (#[cfg(test)] module)

**Checkpoint**: Lookups return correct location types. Blocking on active writers works. Tests pass.

---

## Phase 6: User Story 3 — Committing Staged Data to Persistent Storage (Priority: P2)

**Goal**: Transition staged entries to block-device locations via `convert_to_storage`, freeing the DMA buffer.

**Independent Test**: Stage a key, call convert_to_storage, verify subsequent lookups return BlockDeviceLocation.

### Implementation

- [x] T022 [US3] Implement convert_to_storage(key, offset, block_device_id) — validate entry is Staging, update Location to BlockDevice, remove DMA buffer from side map in src/lib.rs
- [x] T023 [US3] Add doc tests for convert_to_storage in src/lib.rs
- [x] T024 [US3] Unit tests: happy path transition, key-not-found error, non-staging (InvalidState) error in src/lib.rs (#[cfg(test)] module)

**Checkpoint**: Full write flow works: stage → release write → convert to storage → lookup returns BlockDevice. Tests pass.

---

## Phase 7: User Story 5 — Recovery on Initialization (Priority: P2)

**Goal**: On startup, recover all committed extents from `IExtentManager::for_each_extent` and populate the map.

**Independent Test**: Populate a mock extent manager with known extents, initialize the dispatch map, verify all extents appear as BlockDevice entries.

### Implementation

- [x] T025 [US5] Implement initialize() — call extent_manager receptacle's for_each_extent, populate entries as BlockDevice locations, log recovery count via ILogger in src/lib.rs
- [x] T026 [US5] Add doc tests for initialize in src/lib.rs
- [x] T027 [US5] Integration tests: recovery with mock extent manager (populated case — verify all extents in map; empty case — verify empty map) in tests/integration.rs

**Checkpoint**: Recovery populates the map correctly from the extent manager. Tests pass.

---

## Phase 8: User Story 6 — Removing an Extent from the Map (Priority: P3)

**Goal**: Remove entries from the dispatch map, with error on active references.

**Independent Test**: Stage or commit a key with no active refs, call remove, verify lookup returns NotExist.

### Implementation

- [x] T028 [US6] Implement remove(key) — validate no active refs, delete entry from map and buffer from side map, error on active refs or key-not-found in src/lib.rs
- [x] T029 [US6] Add doc tests for remove in src/lib.rs
- [x] T030 [US6] Unit tests: happy path removal, active-refs error, key-not-found error in src/lib.rs (#[cfg(test)] module)

**Checkpoint**: Entries can be removed. Full lifecycle works: stage → convert → remove. Tests pass.

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Logging, benchmarks, lint compliance, final validation

- [x] T031 [P] Add ILogger info/debug/error calls throughout all IDispatchMap method implementations in src/lib.rs
- [x] T032 [P] Implement Criterion benchmarks: lookup latency (no contention), ref op throughput, entry size assertion (≤32 bytes) in benches/dispatch_map_benchmark.rs
- [x] T033 Run cargo fmt -p dispatch-map --check and cargo clippy -p dispatch-map -- -D warnings — fix all issues
- [x] T034 Run cargo doc -p dispatch-map --no-deps — fix all warnings; verify module-level //! docs present in all source files
- [x] T035 Run full test suite with --test-threads 1 to verify single-threaded CI compatibility
- [x] T036 Run quickstart.md validation — verify usage example compiles and described flows work

**Checkpoint**: All quality gates pass. Component is ready for integration.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — BLOCKS all user stories
- **US4 — Ref Counting (Phase 3)**: Depends on Phase 2 — BLOCKS US1, US2, US3
- **US1 — Staging (Phase 4)**: Depends on Phase 3 (needs release_write to test)
- **US2 — Lookup (Phase 5)**: Depends on Phase 3 (needs take_read) and Phase 4 (needs staged entries to look up)
- **US3 — Convert to Storage (Phase 6)**: Depends on Phase 4 (needs staged entries to convert)
- **US5 — Recovery (Phase 7)**: Depends on Phase 2 only (uses mock extent manager, not staging)
- **US6 — Remove (Phase 8)**: Depends on Phase 4 (needs entries to remove)
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Dependencies

```
Phase 2 (Foundational)
    │
    ├──> Phase 3 (US4: Ref Counting)
    │        │
    │        ├──> Phase 4 (US1: Staging)
    │        │        │
    │        │        ├──> Phase 5 (US2: Lookup)
    │        │        ├──> Phase 6 (US3: Convert to Storage)
    │        │        └──> Phase 8 (US6: Remove)
    │        │
    │        └──> Phase 5 (US2: Lookup) [also needs US1]
    │
    └──> Phase 7 (US5: Recovery) [independent of other stories]
```

### Parallel Opportunities

- T006 and T007 (entry.rs and state.rs) are in different files — parallel
- T010 (release_read/write) can parallel with T009 completion if in same file — mark [P] for different logical concern
- US5 (Recovery) can run in parallel with US1/US2/US3 after foundational phase
- T031 (logging) and T032 (benchmarks) are independent — parallel

---

## Parallel Example: Foundational Phase

```
# These can run in parallel (different files):
T006: Implement Location and DispatchEntry in src/entry.rs
T007: Implement DispatchMapState in src/state.rs
```

## Parallel Example: After Phase 3

```
# US5 is independent of other stories:
Phase 4 (US1: Staging)     ←→  Phase 7 (US5: Recovery)
```

---

## Implementation Strategy

### MVP First (US4 + US1 + US2)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational — all types compile
3. Complete Phase 3: US4 — reference counting works
4. Complete Phase 4: US1 — staging works
5. Complete Phase 5: US2 — lookup works
6. **STOP and VALIDATE**: The core read/write path is functional
7. `cargo test -p dispatch-map` passes

### Incremental Delivery

1. Setup + Foundational → types compile
2. US4 (Ref Counting) → concurrency foundation ✓
3. US1 (Staging) → data can be ingested ✓
4. US2 (Lookup) → data can be read ✓ — **MVP complete**
5. US3 (Convert to Storage) → persistence path ✓
6. US5 (Recovery) → crash recovery ✓ (can parallel with US3)
7. US6 (Remove) → cleanup/eviction ✓
8. Polish → quality gates pass

---

## Notes

- [P] tasks = different files or independent concerns, no dependencies
- [Story] label maps task to specific user story for traceability
- Constitution requires doc tests on all public API — included in each story's implementation
- Single Mutex design means implementation tasks for the same file (src/lib.rs) are generally sequential
- Tests use mock DmaAllocFn (returns heap-allocated buffer) — no SPDK hardware needed
- All tests must pass under --test-threads 1 for CI
