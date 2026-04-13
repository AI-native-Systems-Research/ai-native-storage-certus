# Tasks: CUDA GPU Memory Allocation Test for P2P Pinning

**Input**: Design documents from `/specs/002-cuda-pin-test/`
**Prerequisites**: plan.md (required), spec.md (required), research.md,
data-model.md, quickstart.md

**Tests**: This feature IS a test suite. All tasks produce test code.

**Organization**: Tasks are grouped by user story to enable independent
implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2)
- Include exact file paths in descriptions

## Path Conventions

- **Rust crate**: `rust/` at repository root (extends feature 001 crate)
- Test files: `rust/tests/`
- Helper module: `rust/tests/cuda_helpers/mod.rs`

---

## Phase 1: Setup

**Purpose**: Add dependency and create helper module directory structure.

- [X] T001 Add `libloading` as a dev-dependency in `rust/Cargo.toml` under
  `[dev-dependencies]` section.
- [X] T002 [P] Create directory `rust/tests/cuda_helpers/` for the helper
  module used by CUDA integration tests.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Implement the CUDA dlopen helper module that all test functions
depend on. MUST complete before any user story test can run.

**CRITICAL**: No user story work can begin until this phase is complete.

- [X] T003 Create `rust/tests/cuda_helpers/mod.rs` with `SkipReason` enum
  (variants: `NoCudaRuntime`, `NoGpu`, `NoKernelModule`) implementing
  `std::fmt::Display` for human-readable skip messages.
- [X] T004 Implement `CudaRuntime` struct in `rust/tests/cuda_helpers/mod.rs`:
  `load()` method that attempts `dlopen` of `libcudart.so` with fallback chain
  (`libcudart.so` -> `libcudart.so.12` -> `libcudart.so.11.0`), returns
  `Result<CudaRuntime, SkipReason>`. Resolve `cudaMalloc` and `cudaFree`
  symbols using `libloading::Library::get()`.
- [X] T005 Implement `CudaRuntime::malloc(size: usize)` in
  `rust/tests/cuda_helpers/mod.rs`: call resolved `cudaMalloc` symbol, return
  `Result<CudaMemory, SkipReason>` (return `SkipReason::NoGpu` if cudaMalloc
  returns non-zero error).
- [X] T006 Implement `CudaMemory` struct in `rust/tests/cuda_helpers/mod.rs`:
  fields `devptr: *mut c_void`, `size: usize`. Implement `devptr(&self) -> u64`
  accessor (cast to u64). Implement `Drop` to call resolved `cudaFree` symbol.
- [X] T007 Implement `check_prerequisites()` function in
  `rust/tests/cuda_helpers/mod.rs`: load CudaRuntime, attempt 64KB malloc
  (and immediately free), attempt open `/dev/nvidia_p2p` (and immediately
  close). Returns `Result<CudaRuntime, SkipReason>` with first failing
  prerequisite as the skip reason.

**Checkpoint**: Helper module compiles. `check_prerequisites()` returns
`Ok(CudaRuntime)` on a system with CUDA + GPU + kernel module, or
`Err(SkipReason)` with a clear message otherwise.

---

## Phase 3: User Story 1 - Allocate and Pin GPU Memory End-to-End (Priority: P1)

**Goal**: Rust integration test that allocates GPU memory via cudaMalloc, pins
it via the nvidia-p2p-pin library, validates physical addresses, unpins, and
frees. Skips gracefully when prerequisites are missing.

**Independent Test**: `cargo test --test cuda_pin_test test_cuda_pin_1mb`

### Implementation for User Story 1

- [X] T008 [US1] Create `rust/tests/cuda_pin_test.rs` with `mod cuda_helpers;`
  import. Add `test_cuda_pin_1mb` test function: call `check_prerequisites()`,
  skip with println + return on Err. Allocate 1 MB via `CudaRuntime::malloc()`,
  call `NvP2pDevice::open()` then `pin_gpu_memory(devptr, 1MB)`, assert
  page_count == 16 (at 64KB page size), assert all physical_addresses are
  non-zero. Drop PinnedMemory before CudaMemory.
- [X] T009 [US1] Add `test_cuda_pin_64kb_minimum` test function in
  `rust/tests/cuda_pin_test.rs`: allocate 64KB (minimum valid size), pin,
  assert page_count == 1, assert single physical_address is non-zero. Unpin
  and free.
- [X] T010 [US1] Add `test_cuda_pin_unpin_lifecycle` test function in
  `rust/tests/cuda_pin_test.rs`: allocate 1 MB, pin, explicitly call
  `unpin()`, then free GPU memory via CudaMemory Drop. Verify unpin returns
  `Ok(())`. Verify a second pin of the same address succeeds after unpin
  (confirms resources were fully released).
- [X] T011 [US1] Add `test_cuda_skip_no_prerequisites` test function in
  `rust/tests/cuda_pin_test.rs`: verify that when `check_prerequisites()`
  returns Err, the test prints the skip message and returns without panic.
  (This test always passes — it validates the skip mechanism itself.)

**Checkpoint**: All US1 tests pass on a system with prerequisites. Tests skip
cleanly on systems without CUDA/GPU/module.

---

## Phase 4: User Story 2 - Validate Alignment Requirements (Priority: P2)

**Goal**: Rust integration test that verifies cudaMalloc returns 64KB-aligned
pointers compatible with the P2P pinning API.

**Independent Test**: `cargo test --test cuda_pin_test test_cuda_alignment`

### Implementation for User Story 2

- [X] T012 [US2] Add `test_cuda_alignment` test function in
  `rust/tests/cuda_pin_test.rs`: allocate 1 MB via cudaMalloc, assert
  `devptr % 65536 == 0`. Free and repeat for 64KB and 16 MB allocations.
  Skip gracefully if prerequisites missing.
- [X] T013 [US2] Add `test_cuda_multi_size_alignment` test function in
  `rust/tests/cuda_pin_test.rs`: allocate multiple sizes in sequence
  (64KB, 256KB, 1 MB, 4 MB, 16 MB), assert all device pointers are
  64KB-aligned. Free each allocation after checking. Skip gracefully if
  prerequisites missing.

**Checkpoint**: Alignment validated across multiple allocation sizes.

---

## Phase 5: Polish & Cross-Cutting Concerns

**Purpose**: Final validation and cleanup.

- [X] T014 [P] Run `cargo clippy` on `rust/` and fix all warnings in test
  files. Verify unsafe blocks are limited to `cuda_helpers/mod.rs` FFI calls.
- [X] T015 [P] Run full test suite (`sudo cargo test --test cuda_pin_test
  -- --nocapture`) on a system with NVIDIA GPU + CUDA + kernel module. Verify
  all tests pass and output matches quickstart.md expected output.
- [X] T016 Run full test suite on a system WITHOUT CUDA runtime. Verify all
  tests skip gracefully with clear messages and no failures.
- [ ] T017 Verify resource cleanup: run tests under `valgrind` or check
  `/proc/self/fd` counts to confirm no leaked file descriptors or GPU memory
  after test completion.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup (T001 for libloading dep)
- **User Story 1 (Phase 3)**: Depends on Foundational (T003-T007)
- **User Story 2 (Phase 4)**: Depends on Foundational (T003-T007); independent
  of US1
- **Polish (Phase 5)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational — No dependencies on
  other stories
- **User Story 2 (P2)**: Can start after Foundational — Independent of US1
  (uses only CudaRuntime, not pinning)

### Within Each User Story

- All test functions within a story can be implemented in parallel (same file
  but independent functions)
- Each test function is self-contained

### Parallel Opportunities

- T001 and T002 can run in parallel (different files)
- T008, T009, T010, T011 are in the same file but independent functions
- T012 and T013 are in the same file but independent functions
- US1 (Phase 3) and US2 (Phase 4) can proceed in parallel after Foundational
- T014, T015 can run in parallel

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — cuda_helpers module)
3. Complete Phase 3: User Story 1 (end-to-end pin test)
4. **STOP and VALIDATE**: Full lifecycle test passes on GPU system, skips
   elsewhere
5. This is the functional MVP

### Incremental Delivery

1. Setup + Foundational -> cuda_helpers module compiles and loads CUDA
2. Add US1 -> End-to-end pin/unpin lifecycle validated (MVP!)
3. Add US2 -> Alignment assumptions validated across allocation sizes
4. Polish -> Cross-platform skip validation, clippy, resource leak checks

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- All tests require: root/CAP_SYS_RAWIO + NVIDIA GPU + CUDA runtime + loaded
  kernel module (or skip gracefully)
- Drop ordering is critical: PinnedMemory MUST be dropped before CudaMemory
- Commit after each task or logical group
