# Tasks: GPU-to-SSD DMA Buffer Preparation

**Input**: Design documents from `specs/002-gpu-ssd-dma-prepare/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Included — constitution mandates comprehensive unit tests, doc tests, and benchmarks for all public APIs.

**Organization**: Tasks grouped by user story. US1 (core prepare function) and US2 (pin-aware cleanup) are tightly coupled (same function, same free_fn selection logic) so they share a phase.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1, US2, US3)
- Exact file paths included in descriptions

---

## Phase 1: Setup

**Purpose**: Feature gate configuration and dependency verification

- [x] T001 Verify `spdk` feature in components/gpu-services/v0/Cargo.toml enables `interfaces/spdk` (already present — confirm no changes needed)
- [x] T002 Verify `DmaBuffer` is re-exported from `interfaces` crate when `spdk` feature is active in components/interfaces/src/lib.rs

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Interface definition and helper functions that ALL user stories depend on

- [x] T003 Add `prepare_memory_for_spdk` method signature to `IGpuServices` trait in components/interfaces/src/igpu_services.rs with `#[cfg(feature = "spdk")]` gate, doc comment, and `no_run` example
- [x] T004 [P] Add `is_memory_pinned` helper function in components/gpu-services/v0/src/memory.rs that queries `cudaPointerGetAttributes` and returns whether memory is already device-registered for DMA
- [x] T005 [P] Add `cuda_ipc_close_only` free function in components/gpu-services/v0/src/dma.rs — calls only `cudaIpcCloseMemHandle(ptr)`
- [x] T006 [P] Add `cuda_ipc_unpin_and_close` free function in components/gpu-services/v0/src/dma.rs — calls `cudaHostUnregister(ptr)` then `cudaIpcCloseMemHandle(ptr)`
- [x] T007 [P] Add `create_spdk_dma_buffer_from_gpu` function in components/gpu-services/v0/src/dma.rs that accepts `(ptr, size, was_already_pinned)` and returns `Result<interfaces::DmaBuffer, String>` using `DmaBuffer::from_raw` with the appropriate free function

**Checkpoint**: Foundation ready — interface declared, helpers available for implementation

---

## Phase 3: User Story 1+2 — Prepare GPU Memory & Pin-Aware Cleanup (Priority: P1)

**Goal**: Single function call that opens IPC handle, detects pin state, conditionally pins, and returns SPDK DmaBuffer with correct cleanup semantics.

**Independent Test**: Call `prepare_memory_for_spdk` with a valid IPC handle payload, verify DmaBuffer returned has correct size; drop buffer and verify no resource leaks.

### Implementation for User Story 1+2

- [x] T008 [US1] Implement `prepare_memory_for_spdk` method body in components/gpu-services/v0/src/lib.rs under `#[cfg(feature = "spdk")]` — orchestrates: check initialized, optional cudaSetDevice, decode payload, open IPC handle, check pin state, conditionally pin, select free_fn, create DmaBuffer
- [x] T009 [US1] Implement error rollback logic in the `prepare_memory_for_spdk` body: if pinning succeeds but DmaBuffer creation fails, unpin; if IPC open succeeds but later steps fail, close IPC handle
- [x] T010 [US1] Add `#[cfg(not(feature = "gpu"))]` stub for `prepare_memory_for_spdk` that returns "GPU support not compiled" error in components/gpu-services/v0/src/lib.rs
- [x] T011 [P] [US1] Add unit test `test_prepare_memory_not_initialized` in components/gpu-services/v0/src/lib.rs verifying error before init
- [x] T012 [P] [US1] Add unit test `test_prepare_memory_invalid_base64` in components/gpu-services/v0/src/lib.rs verifying invalid payload error
- [x] T013 [P] [US1] Add unit test `test_prepare_memory_wrong_payload_size` in components/gpu-services/v0/src/lib.rs verifying 72-byte constraint
- [ ] T014 [P] [US2] Add unit test `test_free_fn_close_only` in components/gpu-services/v0/src/dma.rs verifying `cuda_ipc_close_only` calls only close
- [ ] T015 [P] [US2] Add unit test `test_free_fn_unpin_and_close` in components/gpu-services/v0/src/dma.rs verifying `cuda_ipc_unpin_and_close` calls unpin then close

**Checkpoint**: Core function complete. DmaBuffer returned with pin-aware cleanup. Error paths roll back cleanly.

---

## Phase 4: User Story 3 — Pin State Logging (Priority: P2)

**Goal**: All pinning decisions logged via logger receptacle when connected.

**Independent Test**: Connect a logger, call `prepare_memory_for_spdk`, verify log messages emitted for pin decision.

### Implementation for User Story 3

- [x] T016 [US3] Add logging calls in `prepare_memory_for_spdk` body: log "memory already pinned — skipping" or "pinning GPU memory for DMA" depending on pin state detection result in components/gpu-services/v0/src/lib.rs
- [x] T017 [US3] Add unit test `test_prepare_memory_logs_pinning_action` in components/gpu-services/v0/src/lib.rs verifying log output with logger connected
- [x] T018 [US3] Add unit test `test_prepare_memory_succeeds_without_logger` in components/gpu-services/v0/src/lib.rs verifying silent success with no logger receptacle

**Checkpoint**: All pinning decisions observable via logger. Operations succeed silently without logger.

---

## Phase 5: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, benchmarks, lint compliance

- [x] T019 [P] Add doc comment with `# Examples` block to `prepare_memory_for_spdk` in components/interfaces/src/igpu_services.rs (must compile as doc test)
- [x] T020 [P] Add Criterion benchmark `bench_prepare_memory_for_spdk` in components/gpu-services/v0/benches/gpu_services_benchmark.rs measuring end-to-end preparation latency
- [x] T021 Run `cargo clippy -p gpu-services --features "gpu,spdk" -- -D warnings` and fix any warnings
- [x] T022 Run `cargo doc -p gpu-services --features "gpu,spdk" --no-deps` and fix any doc warnings
- [x] T023 Run `cargo test -p gpu-services --features "gpu,spdk"` and verify all tests pass
- [x] T024 Validate quickstart.md code examples match final API signature in specs/002-gpu-ssd-dma-prepare/quickstart.md

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — verification only
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories
- **US1+US2 (Phase 3)**: Depends on Foundational (Phase 2)
- **US3 (Phase 4)**: Depends on Phase 3 (logging added to existing function body)
- **Polish (Phase 5)**: Depends on all user stories complete

### User Story Dependencies

- **US1+US2 (P1)**: Can start after Foundational — no dependencies on US3
- **US3 (P2)**: Depends on US1+US2 implementation existing (adds logging to the same function)

### Within Each Phase

- T003 blocks T008 (interface must exist before implementing)
- T004, T005, T006, T007 are all parallel (different functions, different concerns)
- T008 depends on T004, T005, T006, T007 (uses all helpers)
- T009 is sequential after T008 (extends same function)
- T011-T015 are all parallel (independent test functions)
- T016 is sequential after T008 (modifies same function)
- T019-T020 are parallel (different files)

### Parallel Opportunities

Within Phase 2:
- T004, T005, T006, T007 can all run in parallel (separate functions in separate/same files but no conflicts)

Within Phase 3:
- T011, T012, T013, T014, T015 can all run in parallel (independent test functions)

Within Phase 5:
- T019, T020 can run in parallel (different files)

---

## Parallel Example: Phase 2

```
# These can all be implemented simultaneously:
T004: is_memory_pinned helper in src/memory.rs
T005: cuda_ipc_close_only in src/dma.rs
T006: cuda_ipc_unpin_and_close in src/dma.rs
T007: create_spdk_dma_buffer_from_gpu in src/dma.rs
```

## Parallel Example: Phase 3 Tests

```
# These test tasks can all be written simultaneously:
T011: test_prepare_memory_not_initialized
T012: test_prepare_memory_invalid_base64
T013: test_prepare_memory_wrong_payload_size
T014: test_free_fn_close_only
T015: test_free_fn_unpin_and_close
```

---

## Implementation Strategy

### MVP First (US1+US2 Only)

1. Complete Phase 1: Verify feature gates
2. Complete Phase 2: Interface + helpers
3. Complete Phase 3: Core implementation with tests
4. **STOP and VALIDATE**: `cargo test -p gpu-services --features "gpu,spdk"` passes
5. Function works end-to-end with correct cleanup semantics

### Incremental Delivery

1. Setup + Foundational → Interface declared, helpers ready
2. US1+US2 → Core function works with pin-aware cleanup → Validate
3. US3 → Add logging → Validate observability
4. Polish → Benchmarks, docs, lint → Ready for merge

### Key Files Modified

| File | Changes |
|------|---------|
| `components/interfaces/src/igpu_services.rs` | Add method signature + docs |
| `components/gpu-services/v0/src/lib.rs` | Add impl body + tests |
| `components/gpu-services/v0/src/dma.rs` | Add free functions + SPDK buffer creator |
| `components/gpu-services/v0/src/memory.rs` | Add pin-state query helper |
| `components/gpu-services/v0/benches/gpu_services_benchmark.rs` | Add benchmark |

---

## Notes

- Constitution requires tests, doc tests, and benchmarks — all included
- Both `gpu` and `spdk` features must be active for full testing
- Tests that require real GPU hardware use conditional compilation (`#[cfg(feature = "gpu")]`)
- Free functions use `unsafe extern "C"` ABI to match `DmaBuffer::from_raw` signature
- Error rollback (T009) is critical for FR-012 (no resource leaks)
