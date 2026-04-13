# Tasks: NVIDIA P2P GPU Memory Pinning

**Input**: Design documents from `/specs/001-nvidia-p2p-gpu-pin/`
**Prerequisites**: plan.md (required), spec.md (required), research.md,
data-model.md, contracts/ioctl-interface.md, quickstart.md

**Tests**: Included. The constitution requires extensive testing and Criterion
benchmarks for all performance-sensitive code. Integration tests require a
loaded kernel module and NVIDIA GPU.

**Organization**: Tasks are grouped by user story to enable independent
implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **Kernel module**: `kernel/` at repository root
- **Rust library**: `rust/` at repository root
- Paths are relative to repository root

---

## Phase 1: Setup (Project Initialization)

**Purpose**: Create directory structure, build system, and project scaffolding.

- [X] T001 Create `kernel/` directory and `kernel/Makefile` with kbuild
  integration and `nv-p2p.h` auto-discovery via
  `ls -d /usr/src/nvidia-*/nvidia | sort -V | tail -1`; fail with descriptive
  error if not found. Include standard kbuild targets (modules, clean).
- [X] T002 [P] Create `kernel/Kbuild` file declaring `obj-m := nvidia_p2p_pin.o`
- [X] T003 [P] Initialize `rust/Cargo.toml` with crate name `nvidia-p2p-pin`,
  dependencies (`nix` with `ioctl` feature), dev-dependencies (`criterion`),
  and `[[bench]]` section for `pin_unpin` benchmark. Create empty
  `rust/src/lib.rs` placeholder.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Kernel module skeleton and shared ioctl header that ALL user
stories depend on. MUST complete before any user story work begins.

**CRITICAL**: No user story work can begin until this phase is complete.

- [X] T004 Create `kernel/nvidia_p2p_pin.h` with shared ioctl definitions:
  `NVP2P_IOC_MAGIC`, ioctl command macros (`NVP2P_IOCTL_PIN`,
  `NVP2P_IOCTL_UNPIN`, `NVP2P_IOCTL_GET_PAGES`), and all ioctl argument
  structs (`nvp2p_pin_args`, `nvp2p_unpin_args`, `nvp2p_get_pages_args`)
  per contracts/ioctl-interface.md.
- [X] T005 Implement kernel module skeleton in `kernel/nvidia_p2p_pin.c`:
  `module_init` / `module_exit` with `misc_register()` / `misc_deregister()`,
  `struct file_operations` with `.open`, `.release`, `.unlocked_ioctl`,
  `.compat_ioctl`. Device name `nvidia_p2p`, mode `0600`. Add `pr_info` for
  load/unload. Include `MODULE_LICENSE`, `MODULE_AUTHOR`, `MODULE_DESCRIPTION`.
- [X] T006 Implement per-fd state management in `kernel/nvidia_p2p_pin.c`:
  `.open` handler allocates `nvp2p_fd_state` (list_head, mutex, next_handle
  counter), stores in `file->private_data`, checks `CAP_SYS_RAWIO` (return
  `-EPERM` if missing). `.release` handler iterates all regions, calls
  `nvidia_p2p_put_pages_persistent()` for each, frees all regions and fd_state.
- [X] T007 Implement ioctl dispatch stub in `kernel/nvidia_p2p_pin.c`:
  `unlocked_ioctl` handler that switches on command number, calls per-command
  handlers (initially return `-ENOTTY`), and sets `compat_ioctl` to
  `compat_ptr_ioctl`. Add `pr_debug` for ioctl entry/exit tracing.

**Checkpoint**: Module builds, loads (`insmod`), creates `/dev/nvidia_p2p`,
enforces CAP_SYS_RAWIO on open, and can be unloaded (`rmmod`) cleanly.

---

## Phase 3: User Story 1 - Pin GPU Memory for DMA (Priority: P1)

**Goal**: User-space application can pin a GPU VA range and receive physical
addresses for DMA programming.

**Independent Test**: Allocate GPU memory via CUDA, call `pin_gpu_memory()`,
verify physical addresses returned with correct count for region size.

### Tests for User Story 1

> **NOTE: Write tests FIRST, ensure they FAIL before implementation**

- [X] T008 [P] [US1] Write integration test `test_pin_valid_region` in
  `rust/tests/integration.rs`: allocate 1 MB via CUDA, pin via library,
  assert page_count == 16 (at 64KB page size), assert all physical_addresses
  are non-zero. Requires `#[ignore]` attribute until kernel module is ready.
- [X] T009 [P] [US1] Write integration test `test_pin_invalid_alignment` in
  `rust/tests/integration.rs`: attempt to pin a non-64KB-aligned address,
  assert `InvalidAlignment` error returned.
- [X] T010 [P] [US1] Write integration test `test_pin_invalid_length` in
  `rust/tests/integration.rs`: attempt to pin with length not a multiple of
  64KB, assert `InvalidLength` error returned.
- [X] T011 [P] [US1] Write integration test `test_pin_duplicate_rejected` in
  `rust/tests/integration.rs`: pin a region, attempt to pin the same range
  again, assert `AlreadyPinned` error returned.

### Implementation for User Story 1

- [X] T012 [P] [US1] Create `rust/src/error.rs` with `Error` enum: variants
  `DeviceNotFound`, `PermissionDenied`, `InvalidAlignment`, `InvalidLength`,
  `InvalidHandle`, `AlreadyPinned`, `OutOfMemory`, `DriverError(i32)`,
  `IoError(std::io::Error)`. Implement `std::fmt::Display` and
  `std::error::Error`. Map kernel errno values to variants.
- [X] T013 [P] [US1] Create `rust/src/ioctl.rs` with ioctl struct definitions
  (`NvP2pPinArgs`, `NvP2pUnpinArgs`, `NvP2pGetPagesArgs`) matching the C
  header layout exactly. Define ioctl command constants using `nix` macros
  (`ioctl_readwrite!`, `ioctl_write_ptr!`). This is the `unsafe` boundary.
- [X] T014 [US1] Implement `NVP2P_IOCTL_PIN` handler in
  `kernel/nvidia_p2p_pin.c`: validate VA alignment and length, acquire mutex,
  scan region list for overlap (reject with `-EEXIST`), call
  `nvidia_p2p_get_pages_persistent()`, allocate `nvp2p_region`, assign handle,
  add to list, fill output args, release mutex. Add `pr_debug` tracing and
  `pr_err` for error paths.
- [X] T015 [US1] Implement `NVP2P_IOCTL_GET_PAGES` handler in
  `kernel/nvidia_p2p_pin.c`: acquire mutex, find region by handle, compute
  min(entries, buf_count), `copy_to_user()` physical addresses to user buffer,
  fill entries_written/page_size/gpu_uuid, release mutex. Return `-EFAULT` on
  bad user pointer.
- [X] T016 [US1] Create `rust/src/device.rs` with `NvP2pDevice` struct:
  `open()` constructor (open `/dev/nvidia_p2p`, map errno to Error),
  `pin_gpu_memory(va, len)` method (call IOCTL_PIN, allocate Vec, call
  IOCTL_GET_PAGES, construct `PinnedMemory`).
- [X] T017 [US1] Create `rust/src/lib.rs` with `PinnedMemory` struct:
  fields per data-model (device, handle, virtual_address, length, page_size,
  page_count, physical_addresses, unpinned). Implement accessors:
  `physical_addresses()`, `physical_address()` (first entry), `page_size()`,
  `page_count()`. Re-export public API (`NvP2pDevice`, `PinnedMemory`,
  `PageSize`, `Error`).
- [X] T018 [US1] Add Criterion benchmark `bench_pin` in
  `rust/benches/pin_unpin.rs`: benchmark `pin_gpu_memory()` latency for
  64KB, 1 MB, and 16 MB regions. Add benchmark group `pin_latency`.

**Checkpoint**: `pin_gpu_memory()` returns physical addresses. Integration tests
for pin pass. Criterion benchmark runs and reports latency.

---

## Phase 4: User Story 2 - Unpin Previously Pinned GPU Memory (Priority: P1)

**Goal**: User-space application can release pinned GPU memory regions, and
regions are automatically released on handle drop or process exit.

**Independent Test**: Pin a region, unpin it, verify success. Attempt
double-unpin, verify `InvalidHandle` error.

### Tests for User Story 2

- [X] T019 [P] [US2] Write integration test `test_unpin_valid` in
  `rust/tests/integration.rs`: pin a region, call `unpin()`, assert success.
- [X] T020 [P] [US2] Write integration test `test_unpin_double_free` in
  `rust/tests/integration.rs`: pin a region, unpin it, attempt to unpin the
  same handle again via raw ioctl, assert `InvalidHandle` error.
- [X] T021 [P] [US2] Write integration test `test_drop_auto_unpin` in
  `rust/tests/integration.rs`: pin a region, drop the `PinnedMemory` handle,
  verify no resource leak (pin a new region to confirm module still functional).

### Implementation for User Story 2

- [X] T022 [US2] Implement `NVP2P_IOCTL_UNPIN` handler in
  `kernel/nvidia_p2p_pin.c`: acquire mutex, find region by handle (return
  `-EINVAL` if not found), remove from list, release mutex, call
  `nvidia_p2p_put_pages_persistent()`, kfree region. Add `pr_debug` tracing.
- [X] T023 [US2] Implement `PinnedMemory::unpin(self)` in `rust/src/lib.rs`:
  call IOCTL_UNPIN with stored handle, set `unpinned = true`, consume self.
  Implement `Drop` for `PinnedMemory`: if not already unpinned, issue
  best-effort IOCTL_UNPIN (log warning on failure via `eprintln!`).
- [X] T024 [US2] Add Criterion benchmarks in `rust/benches/pin_unpin.rs`:
  `bench_unpin` (unpin latency), `bench_pin_unpin_roundtrip` (combined
  pin+unpin cycle). Add benchmark group `unpin_latency`.

**Checkpoint**: Full pin/unpin cycle works. Drop auto-unpins. Double-unpin
returns error. Criterion benchmarks for unpin latency available.

---

## Phase 5: User Story 3 - Query Pinned Region Metadata (Priority: P2)

**Goal**: User-space application can query metadata (page count, page size,
physical addresses, GPU UUID) for a pinned region handle.

**Independent Test**: Pin a region, call query, verify page_count and gpu_uuid
match expected values.

### Tests for User Story 3

- [X] T025 [P] [US3] Write integration test `test_query_metadata` in
  `rust/tests/integration.rs`: pin a 1 MB region, call
  `query_pinned_region()`, assert page_count == 16, assert gpu_uuid is
  non-zero, assert physical_addresses match those from pin.
- [X] T026 [P] [US3] Write integration test `test_query_invalid_handle` in
  `rust/tests/integration.rs`: call `query_pinned_region()` with handle 0xDEAD,
  assert `InvalidHandle` error.

### Implementation for User Story 3

- [X] T027 [US3] Add `query_pinned_region(handle)` method to `NvP2pDevice`
  in `rust/src/device.rs`: call IOCTL_GET_PAGES with the given handle and
  return a `RegionMetadata` struct containing page_count, page_size,
  physical_addresses, and gpu_uuid.
- [X] T028 [US3] Add `RegionMetadata` struct in `rust/src/lib.rs` with fields:
  `page_count: u32`, `page_size: PageSize`, `physical_addresses: Vec<u64>`,
  `gpu_uuid: [u8; 16]`. Re-export from crate root.

**Checkpoint**: All 3 user stories independently functional and tested.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Improvements that affect multiple user stories.

- [X] T029 [P] Verify kernel module builds against RHEL 9 kernel 5.14 headers
  and RHEL 10 headers. Document any compatibility issues in
  `specs/001-nvidia-p2p-gpu-pin/research.md`.
- [X] T030 [P] Run `cargo clippy` on `rust/` and fix all warnings. Verify zero
  `unsafe` blocks outside `rust/src/ioctl.rs`.
- [X] T031 [P] Run all Criterion benchmarks (`cargo bench`) and record baseline
  results in `specs/001-nvidia-p2p-gpu-pin/research.md` under a new section
  "Benchmark Results".
- [ ] T032 Verify module cleanup: load module, pin multiple regions from
  multiple processes, kill processes, unload module. Check `dmesg` for clean
  release messages and no warnings. Validate with `kmemleak` if available.
- [ ] T033 Run `quickstart.md` validation: follow the quickstart guide
  end-to-end on a fresh environment, confirm all steps succeed.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Stories (Phase 3+)**: All depend on Foundational phase completion
  - US1 (Pin) can start after Foundational
  - US2 (Unpin) depends on US1 kernel PIN handler (T014) being complete
  - US3 (Query) can start after Foundational (GET_PAGES handler from US1 T015)
- **Polish (Phase 6)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational (Phase 2)
- **User Story 2 (P1)**: Depends on US1 kernel handlers (T014, T015) for
  pin operations used in testing. Rust-side (T012, T013) can be reused.
- **User Story 3 (P2)**: Depends on US1 kernel GET_PAGES handler (T015)
  and Rust ioctl definitions (T013)

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Rust error.rs and ioctl.rs before device.rs
- device.rs before lib.rs
- Kernel handlers before Rust device layer
- Core implementation before benchmarks

### Parallel Opportunities

- T002 and T003 can run in parallel (different directories)
- T008, T009, T010, T011 can run in parallel (same file, independent tests)
- T012 and T013 can run in parallel (different Rust files)
- T019, T020, T021 can run in parallel (independent tests)
- T025 and T026 can run in parallel (independent tests)
- All Phase 6 tasks marked [P] can run in parallel

---

## Implementation Strategy

### MVP First (User Stories 1 + 2)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: User Story 1 (Pin)
4. Complete Phase 4: User Story 2 (Unpin)
5. **STOP and VALIDATE**: Full pin/unpin cycle with integration tests and
   Criterion benchmarks
6. Deploy/demo if ready — this is the functional MVP

### Incremental Delivery

1. Setup + Foundational -> Module loads and creates `/dev/nvidia_p2p`
2. Add US1 (Pin) -> Can pin and read physical addresses
3. Add US2 (Unpin) -> Full pin/unpin lifecycle, RAII via Drop (MVP!)
4. Add US3 (Query) -> Metadata inspection for diagnostics
5. Polish -> Cross-platform validation, benchmarks, cleanup verification

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together
2. Once Foundational is done:
   - Developer A: Kernel ioctl handlers (T014, T015, T022)
   - Developer B: Rust library (T012, T013, T016, T017, T023)
3. Integration tests run once both sides are ready

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Integration tests require: loaded kernel module + NVIDIA GPU + CUDA runtime
- Criterion benchmarks require same environment as integration tests
- All kernel code must compile with `-Werror` under kbuild
- Commit after each task or logical group
