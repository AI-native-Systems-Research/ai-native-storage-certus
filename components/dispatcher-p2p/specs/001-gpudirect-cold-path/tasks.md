# Tasks: GPUDirect Storage Cold Path

**Input**: Design documents from `specs/001-gpudirect-cold-path/`

**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Tests are included — the constitution requires unit tests and doc tests for all public APIs.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup

**Purpose**: Project initialization and Cargo.toml configuration

- [x] T001 Create Cargo.toml with dependencies: gpu-services (p2p feature), block-device-spdk-nvme (optional), extent-manager, memory-tier, spdk-env, component-framework, interfaces (spdk feature) in Cargo.toml
- [x] T002 [P] Create src/lib.rs with `define_component!` macro declaring DispatcherP2pComponent with receptacles: logger, dispatch_map, gpu_services, spdk_env, memory_tier in src/lib.rs
- [x] T003 [P] Create src/io_segmenter.rs with I/O chunking utility (break large reads into sector-aligned chunks) in src/io_segmenter.rs

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core infrastructure that MUST be complete before any user story

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 Implement P2pRing struct in src/p2p_ring.rs: `new()` allocates 64 slots via gpu-services `allocate_pinned_dma_buffer`, creates 2 CUDA streams; returns `Option<P2pRing>` (None on failure) in src/p2p_ring.rs
- [x] T005 Implement P2pRing `Drop` in src/p2p_ring.rs: free all DmaBuffers, destroy CUDA streams via `destroy()` method in src/p2p_ring.rs
- [x] T006 Implement ThreadPartition helper in src/p2p_ring.rs: compute ring_offset and effective_qd from thread index and total_slots (max 16 per thread) in src/p2p_ring.rs
- [x] T007 Implement PipelineRing struct (DRAM fallback) in src/pipeline.rs: pre-allocate CUDA-pinned DMA buffers and 2 CUDA streams for standard SSD→DRAM→GPU path in src/pipeline.rs
- [ ] T008 Implement PathSelection enum and OnceLock initialization in src/lib.rs: attempt P2pRing::new(), store P2p or DramFallback variant immutably in src/lib.rs
- [x] T009 [P] Implement DataDrive struct and BlockDeviceFactory/ExtentManagerFactory in src/lib.rs for managing block device + extent manager pairs in src/lib.rs
- [x] T010 [P] Implement background.rs: BackgroundEvictor (evicts LRU entries from memory-tier to SSD) and ParallelBackgroundWriter (writes pending entries to SSD) in src/background.rs

**Checkpoint**: Foundation ready — P2P ring, DRAM pipeline, path selection, and drive management all available

---

## Phase 3: User Story 1 — Cold Lookup via P2P Path (Priority: P1)

**Goal**: Cold lookups read from SSD directly into GPU staging buffers, D2D copy to client, promote back to DRAM

**Independent Test**: Evict entries, issue lookups, verify correct data at client GPU via P2P path

### Tests for User Story 1

- [ ] T011 [P] [US1] Unit test for pipelined_ssd_to_gpu_p2p: mock NVMe reads and verify D2D copy sequence and stream sync in src/pipeline.rs (test module)
- [x] T012 [P] [US1] Unit test for P2pRing ThreadPartition: verify non-overlapping partitions, effective_qd bounds, edge cases in src/p2p_ring.rs (test module)
- [ ] T013 [P] [US1] Doc test for P2pRing::new() showing initialization pattern in src/p2p_ring.rs

### Implementation for User Story 1

- [x] T014 [US1] Implement `pipelined_ssd_to_gpu_p2p` function in src/pipeline.rs: prime NVMe reads into P2P ring slots, on completion issue D2D copy on alternating stream, submit next read, sync every ring_size/2 completions in src/pipeline.rs
- [x] T015 [US1] Implement `lookup` method (IDispatcher) cold-path branch in src/lib.rs: when dispatch_map returns BlockDevice, compute ThreadPartition, call pipelined_ssd_to_gpu_p2p, then promote entry to memory-tier in src/lib.rs
- [x] T016 [US1] Implement `lookup_async` method returning GpuStream for async cold lookups in src/lib.rs
- [x] T017 [US1] Implement `batch_lookup` method with concurrent cold lookups using thread partitioning in src/lib.rs
- [x] T018 [US1] Handle NVMe read failure in pipeline: return IoError, recycle slot, do not corrupt other in-flight operations in src/pipeline.rs
- [x] T019 [US1] Handle D2D copy failure: propagate error to caller, recycle ring slot cleanly in src/pipeline.rs

**Checkpoint**: Single-client and multi-client cold lookups work correctly via P2P path

---

## Phase 4: User Story 2 — Fallback to DRAM When P2P Unavailable (Priority: P2)

**Goal**: Component works correctly on hardware without P2P support, using DRAM bounce path

**Independent Test**: Simulate P2P ring failure, verify cold lookups complete via DRAM path

### Tests for User Story 2

- [x] T020 [P] [US2] Unit test for DRAM fallback path: mock P2pRing::new() failure, verify cold lookup uses PipelineRing and completes correctly in src/lib.rs (test module)
- [ ] T021 [P] [US2] Unit test for P2pRing::new() failure modes: GDRCopy unavailable, insufficient GPU memory, partial allocation cleanup in src/p2p_ring.rs (test module)

### Implementation for User Story 2

- [x] T022 [US2] Implement `pipelined_ssd_to_gpu_dram` function in src/pipeline.rs: NVMe read into PipelineRing DRAM buffer, then H2D copy to client GPU in src/pipeline.rs
- [x] T023 [US2] Wire DRAM fallback into lookup/lookup_async/batch_lookup: when PathSelection is DramFallback, call pipelined_ssd_to_gpu_dram instead of P2P variant in src/lib.rs
- [x] T024 [US2] Ensure P2pRing::new() cleans up partial allocations on failure (free any slots already allocated before the failure point) in src/p2p_ring.rs
- [ ] T025 [US2] Add startup log messages: "P2P ring initialized (64 slots)" or "P2P ring initialization failed: {reason}, falling back to DRAM path" in src/lib.rs

**Checkpoint**: Component operates correctly with or without P2P hardware

---

## Phase 5: User Story 3 — Hot Path Unaffected (Priority: P2)

**Goal**: Hot lookups (entries in DRAM) have identical performance to standard dispatcher

**Independent Test**: Measure hot-path throughput, verify no regression from P2P machinery

### Tests for User Story 3

- [x] T026 [P] [US3] Unit test for hot-path lookup: when dispatch_map returns MemoryTier, verify DMA copy from DRAM to client GPU without touching P2P ring in src/lib.rs (test module)

### Implementation for User Story 3

- [x] T027 [US3] Implement hot-path branch in lookup/lookup_async/batch_lookup: when dispatch_map returns MemoryTier, DMA copy from memory-tier pointer to client GPU via gpu_services (identical to standard dispatcher) in src/lib.rs
- [x] T028 [US3] Verify hot path does not acquire any P2P ring resources or synchronize CUDA streams in src/lib.rs

**Checkpoint**: Hot and cold paths are independent; hot path has zero P2P overhead

---

## Phase 6: User Story 4 — Performance Is Measurable (Priority: P3)

**Goal**: End-to-end performance can be measured and compared between P2P and DRAM paths

**Independent Test**: Run benchmark tool, observe throughput numbers for both paths

### Implementation for User Story 4

- [ ] T029 [P] [US4] Create Criterion benchmark in benches/cold_path_benchmark.rs: measure pipeline throughput for P2P path (requires hardware feature gate) in benches/cold_path_benchmark.rs
- [ ] T030 [P] [US4] Create Criterion benchmark for DRAM fallback path in same bench file for comparison in benches/cold_path_benchmark.rs
- [ ] T031 [US4] Verify certus-api-bench_v2.py works end-to-end with the full-p2p.yaml profile: start server, populate, evict, measure cold throughput in apps/python/certus-api-bench_v2.py (validation only, no code changes expected)

**Checkpoint**: Throughput is measurable for both paths; operator can compare results

---

## Phase 7: Remaining IDispatcher Methods

**Purpose**: Complete the IDispatcher interface implementation (non-cold-path methods)

- [x] T032 [P] Implement `initialize(config)` in src/lib.rs: set up data drives, init dispatch map, memory tier, background writer/evictor, then path selection in src/lib.rs
- [x] T033 [P] Implement `shutdown()` in src/lib.rs: stop background tasks, drain writes, release P2P ring or PipelineRing, disconnect drives in src/lib.rs
- [x] T034 [P] Implement `check`, `remove`, `touch` methods (dispatch-map delegation) in src/lib.rs
- [x] T035 [P] Implement `populate` method (client GPU → memory-tier → SSD write-through) in src/lib.rs
- [x] T036 [P] Implement `prepare_store`, `commit_store`, `cancel_store` methods (staging buffer workflow) in src/lib.rs
- [x] T037 [P] Implement `clear_memory_tier` and `flush_to_ssd` methods in src/lib.rs
- [x] T038 Unit tests for initialize/shutdown lifecycle (verify resource cleanup, no leaks) in src/lib.rs (test module)
- [ ] T039 Doc tests for public IDispatcher methods showing usage patterns in src/lib.rs

**Checkpoint**: Full IDispatcher contract implemented and tested

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, final validation, cleanup

- [x] T040 [P] Add module-level documentation to all source files (lib.rs, p2p_ring.rs, pipeline.rs, background.rs, io_segmenter.rs)
- [x] T041 [P] Add `// SAFETY:` comments to all unsafe blocks (DMA operations, raw pointer handling)
- [x] T042 Run `cargo clippy -p dispatcher-p2p -- -D warnings` and fix any warnings
- [ ] T043 Run `cargo doc -p dispatcher-p2p --no-deps` and fix any doc warnings
- [x] T044 Run `cargo fmt -p dispatcher-p2p --check` and fix formatting
- [ ] T045 Run full quickstart.md validation scenarios on hardware

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Foundational (Phase 2)
- **User Story 2 (Phase 4)**: Depends on Foundational (Phase 2), can run parallel to US1
- **User Story 3 (Phase 5)**: Depends on Foundational (Phase 2), can run parallel to US1/US2
- **User Story 4 (Phase 6)**: Depends on US1 and US2 (needs both paths working to measure)
- **Remaining Methods (Phase 7)**: Depends on Foundational (Phase 2), can run parallel to US1-US3
- **Polish (Phase 8)**: Depends on all prior phases

### User Story Dependencies

- **US1 (P1)**: After Foundational — No dependencies on other stories
- **US2 (P2)**: After Foundational — Independent of US1 (different code path)
- **US3 (P2)**: After Foundational — Independent of US1/US2 (different code path)
- **US4 (P3)**: After US1 + US2 (needs both paths operational to measure)

### Within Each User Story

- Tests written alongside or before implementation
- Models/types before algorithms
- Core pipeline function before IDispatcher wiring
- Error handling after happy path

### Parallel Opportunities

- T002, T003: Setup tasks on different files
- T009, T010: Independent infrastructure modules
- T011, T012, T013: Independent test tasks for US1
- T020, T021: Independent test tasks for US2
- T029, T030: Independent benchmark files for US4
- T032–T037: Independent IDispatcher methods
- T040–T044: Independent polish tasks

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (P2P ring + DRAM pipeline + path selection)
3. Complete Phase 3: User Story 1 (P2P cold lookup)
4. **STOP and VALIDATE**: Test cold lookups on hardware
5. This delivers the core value proposition

### Incremental Delivery

1. Setup + Foundational → Infrastructure ready
2. Add US1 → P2P cold path works → Core value delivered
3. Add US2 → DRAM fallback works → Deployable without P2P hardware
4. Add US3 → Hot path verified → Full dispatcher replacement ready
5. Add US4 → Performance measurable → Can evaluate P2P value
6. Add Phase 7 → Full IDispatcher → Production-ready component
7. Polish → Documentation and lint gates pass

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story
- Each user story is independently testable after Foundational phase
- Hardware-dependent tests (Criterion benchmarks, certus-api-bench) gated behind `hardware-test` feature
- Mock-based tests run in CI without hardware
