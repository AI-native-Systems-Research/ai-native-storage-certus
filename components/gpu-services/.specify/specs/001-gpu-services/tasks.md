# Tasks: GPU Services Component

**Input**: Design documents from `/specs/001-gpu-services/`
**Prerequisites**: plan.md (required), spec.md (required for user stories)
**Status**: Backfilled from existing implementation. All tasks are COMPLETE.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization, feature gates, and build system

- [x] T001 Create crate skeleton with `Cargo.toml` defining feature gates (`gpu`, `spdk`, `p2p`) at `components/gpu-services/Cargo.toml`
- [x] T002 Implement `build.rs` with conditional link search paths for `libcudart` and `libgdrapi` at `components/gpu-services/build.rs`
- [x] T003 [P] Define `IGpuServices` interface trait with `define_interface!` macro at `components/interfaces/src/igpu_services.rs`
- [x] T004 [P] Define shared types (`GpuDeviceInfo`, `GpuIpcHandle`, `GpuDmaBuffer`, `GpuStream`) at `components/interfaces/src/igpu_services.rs`
- [x] T005 [P] Implement `GpuServicesComponent` with `define_component!`, `ILogger` receptacle, and `Mutex<GpuState>` at `components/gpu-services/src/lib.rs`
- [x] T006 [P] Implement graceful degradation stubs for all methods when `gpu` feature is disabled at `components/gpu-services/src/lib.rs`

**Checkpoint**: Crate compiles without CUDA libraries, all operations return "GPU support not compiled" errors.

---

## Phase 2: Foundational (CUDA FFI + Device Discovery)

**Purpose**: Core CUDA bindings and GPU hardware discovery that all higher-level operations depend on

- [x] T007 Implement hand-written CUDA runtime FFI bindings (minimal API surface: ~25 functions) at `components/gpu-services/src/cuda_ffi.rs`
- [x] T008 [P] Implement `cuda_error_string()` helper for translating CUDA error codes to Rust strings at `components/gpu-services/src/cuda_ffi.rs`
- [x] T009 Implement `discover_devices()` function: enumerate GPUs, filter by compute capability >= 7.0, extract PCI bus IDs at `components/gpu-services/src/device.rs`
- [x] T010 Wire `initialize()` to call `discover_devices()` and store results in `GpuState` at `components/gpu-services/src/lib.rs`
- [x] T011 [P] Implement `shutdown()` clearing all state (devices, verified set, pinned set) at `components/gpu-services/src/lib.rs`
- [x] T012 [P] Implement `get_devices()` with initialization guard at `components/gpu-services/src/lib.rs`

**Checkpoint**: `initialize()` discovers GPUs; `get_devices()` returns device info; `shutdown()` resets state.

---

## Phase 3: User Story 1 - GPU Device Discovery (Priority: P1)

**Goal**: Confirm hardware readiness via CUDA initialization and device enumeration

**Independent Test**: `cargo test -p gpu-services --features gpu test_initialize_idempotent test_shutdown_releases_state`

### Tests for User Story 1

- [x] T013 [P] [US1] Unit test: `test_provides_igpu_services` verifies IGpuServices query at `components/gpu-services/src/lib.rs`
- [x] T014 [P] [US1] Unit test: `test_initialize_without_logger` verifies graceful behavior at `components/gpu-services/src/lib.rs`
- [x] T015 [P] [US1] Unit test: `test_shutdown_without_logger` verifies shutdown always succeeds at `components/gpu-services/src/lib.rs`
- [x] T016 [P] [US1] Unit test: `test_get_devices_before_init_fails` verifies pre-init guard at `components/gpu-services/src/lib.rs`
- [x] T017 [P] [US1] Unit test: `test_initialize_with_logger` verifies logger receptacle integration at `components/gpu-services/src/lib.rs`
- [x] T018 [P] [US1] Unit test: `test_initialize_idempotent` verifies second call returns Ok at `components/gpu-services/src/lib.rs`
- [x] T019 [P] [US1] Unit test: `test_shutdown_releases_state` verifies get_devices fails after shutdown at `components/gpu-services/src/lib.rs`

### Implementation for User Story 1

- [x] T020 [US1] Implement idempotent `initialize()` with early return when already initialized at `components/gpu-services/src/lib.rs`
- [x] T021 [US1] Implement PCI bus ID extraction via `cudaDeviceGetPCIBusId` in device discovery at `components/gpu-services/src/device.rs`
- [x] T022 [US1] Add ILogger info messages for init/device-count at `components/gpu-services/src/lib.rs`

**Checkpoint**: Device discovery works, returns GpuDeviceInfo with all fields populated.

---

## Phase 4: User Story 2 - IPC Handle Deserialization (Priority: P1)

**Goal**: Receive and open CUDA IPC memory handles from inference clients

**Independent Test**: `cargo test -p gpu-services --features gpu test_deserialize_invalid_base64 test_deserialize_wrong_payload_size`

### Tests for User Story 2

- [x] T023 [P] [US2] Unit test: `test_deserialize_invalid_base64` at `components/gpu-services/src/lib.rs`
- [x] T024 [P] [US2] Unit test: `test_deserialize_wrong_payload_size` at `components/gpu-services/src/lib.rs`

### Implementation for User Story 2

- [x] T025 [US2] Implement `decode_ipc_payload()`: base64 decode, validate 72 bytes, extract handle + size at `components/gpu-services/src/ipc.rs`
- [x] T026 [US2] Implement `open_ipc_handle()`: construct `cudaIpcMemHandle_t`, call `cudaIpcOpenMemHandle`, return `GpuIpcHandle` at `components/gpu-services/src/ipc.rs`
- [x] T027 [US2] Wire `deserialize_ipc_handle()` in component: initialization guard, decode, open, log at `components/gpu-services/src/lib.rs`

**Checkpoint**: Valid base64 IPC payloads produce GpuIpcHandle; invalid inputs return descriptive errors.

---

## Phase 5: User Story 3 - Memory Verification and Pinning (Priority: P1)

**Goal**: Verify GPU memory is device-type and track pin state for DMA readiness

**Independent Test**: `cargo test -p gpu-services --features gpu` (all memory state tests)

### Implementation for User Story 3

- [x] T028 [US3] Implement `check_memory_attributes()`: `cudaPointerGetAttributes`, verify `cudaMemoryTypeDevice` at `components/gpu-services/src/memory.rs`
- [x] T029 [US3] Implement `verify_memory()`: call check_memory_attributes, insert into verified set at `components/gpu-services/src/lib.rs`
- [x] T030 [US3] Implement `pin_memory()`: idempotent, implicit verify if not verified, insert into pinned set at `components/gpu-services/src/lib.rs`
- [x] T031 [US3] Implement `unpin_memory()`: remove from pinned set, error if not pinned at `components/gpu-services/src/lib.rs`
- [x] T032 [US3] Implement `create_dma_buffer()`: reject if not verified+pinned, create GpuDmaBuffer with IPC close free_fn at `components/gpu-services/src/lib.rs`
- [x] T033 [P] [US3] Implement `cuda_ipc_close_mem_handle` free function for GpuDmaBuffer drop at `components/gpu-services/src/dma.rs`
- [x] T034 [P] [US3] Implement `create_gpu_dma_buffer()` helper: null check, construct GpuDmaBuffer at `components/gpu-services/src/dma.rs`

**Checkpoint**: Full handle lifecycle (deserialize → verify → pin → create_dma_buffer → drop) works correctly.

---

## Phase 6: User Story 4 - SPDK DMA Buffer Preparation (Priority: P1)

**Goal**: One-call API to prepare GPU IPC memory for NVMe DMA with full error rollback

**Independent Test**: `cargo test -p gpu-services --features gpu,spdk test_prepare_memory`

### Tests for User Story 4

- [x] T035 [P] [US4] Unit test: `test_prepare_memory_not_initialized` at `components/gpu-services/src/lib.rs`
- [x] T036 [P] [US4] Unit test: `test_prepare_memory_invalid_base64` at `components/gpu-services/src/lib.rs`
- [x] T037 [P] [US4] Unit test: `test_prepare_memory_wrong_payload_size` at `components/gpu-services/src/lib.rs`
- [x] T038 [P] [US4] Unit test: `test_prepare_memory_succeeds_without_logger` at `components/gpu-services/src/lib.rs`
- [x] T039 [P] [US4] Unit test: `test_prepare_memory_logs_with_logger` at `components/gpu-services/src/lib.rs`

### Implementation for User Story 4

- [x] T040 [US4] Implement `REGISTERED_REGIONS` static (OnceLock<Mutex<HashMap>>) for size tracking at `components/gpu-services/src/dma.rs`
- [x] T041 [US4] Implement `spdk_unregister_and_ipc_close` free function (already-pinned path) at `components/gpu-services/src/dma.rs`
- [x] T042 [US4] Implement `spdk_unregister_unpin_and_ipc_close` free function (we-pinned path) at `components/gpu-services/src/dma.rs`
- [x] T043 [US4] Implement `create_spdk_dma_buffer_from_gpu()`: spdk_mem_register, select free_fn, DmaBuffer::from_raw at `components/gpu-services/src/dma.rs`
- [x] T044 [US4] Implement `prepare_memory_for_spdk()`: device context switch, decode, open, pin-state check, rollback on error at `components/gpu-services/src/lib.rs`
- [x] T045 [US4] Implement device context save/restore helper (`cudaGetDevice`/`cudaSetDevice`) at `components/gpu-services/src/lib.rs`

**Checkpoint**: `prepare_memory_for_spdk()` returns DmaBuffer ready for NVMe; errors roll back cleanly.

---

## Phase 7: User Story 5 - Synchronous DMA Transfers (Priority: P1)

**Goal**: Copy data between GPU VRAM and SPDK DMA buffers

**Independent Test**: `cargo test -p gpu-services --features gpu test_dma_cpu_to_gpu_roundtrip`

### Tests for User Story 5

- [x] T046 [US5] Unit test: `test_dma_cpu_to_gpu_roundtrip` verifies CPU→GPU→CPU data integrity at `components/gpu-services/src/lib.rs`

### Implementation for User Story 5

- [x] T047 [US5] Implement `dma_copy_to_host()`: size validation, cudaMemcpy D2H at `components/gpu-services/src/lib.rs`
- [x] T048 [US5] Implement `dma_copy_to_device()`: size validation, cudaMemcpy H2D at `components/gpu-services/src/lib.rs`

**Checkpoint**: Synchronous memcpy works in both directions with proper size validation.

---

## Phase 8: User Story 6 - Asynchronous DMA Transfers (Priority: P2)

**Goal**: Issue non-blocking GPU memory copies on CUDA streams for pipeline overlap

**Independent Test**: Stream create/query/sync/destroy + async copy with synchronization

### Implementation for User Story 6

- [x] T049 [US6] Implement `create_stream()`: cudaStreamCreate wrapper with init guard at `components/gpu-services/src/lib.rs`
- [x] T050 [US6] Implement `destroy_stream()`: cudaStreamDestroy wrapper at `components/gpu-services/src/lib.rs`
- [x] T051 [US6] Implement `stream_query()`: cudaStreamQuery, map cudaErrorNotReady to Ok(false) at `components/gpu-services/src/lib.rs`
- [x] T052 [US6] Implement `stream_synchronize()`: cudaStreamSynchronize wrapper at `components/gpu-services/src/lib.rs`
- [x] T053 [US6] Implement `dma_copy_to_device_async()`: size validation, cudaMemcpyAsync H2D on stream at `components/gpu-services/src/lib.rs`
- [x] T054 [US6] Implement `dma_copy_to_host_async()`: size validation, cudaMemcpyAsync D2H on stream at `components/gpu-services/src/lib.rs`
- [x] T055 [P] [US6] Implement `memcpy_h2d_async()`: raw pointer variant without DmaBuffer wrapper at `components/gpu-services/src/lib.rs`
- [x] T056 [P] [US6] Implement `memcpy_d2h_async()`: raw pointer variant without DmaBuffer wrapper at `components/gpu-services/src/lib.rs`

**Checkpoint**: Async copies enqueue without blocking; stream_synchronize waits for completion.

---

## Phase 9: User Story 7 - Pinned Host Memory (Priority: P2)

**Goal**: Allocate page-locked host memory registered with both CUDA and SPDK

### Implementation for User Story 7

- [x] T057 [US7] Implement `spdk_unregister_and_cuda_free_host` free function at `components/gpu-services/src/dma.rs`
- [x] T058 [US7] Implement `create_spdk_dma_buffer_from_cuda_host_alloc()`: spdk_mem_register + DmaBuffer at `components/gpu-services/src/dma.rs`
- [x] T059 [US7] Implement `allocate_pinned_dma_buffer()`: cudaHostAlloc + create_spdk_dma_buffer_from_cuda_host_alloc at `components/gpu-services/src/lib.rs`
- [x] T060 [US7] Implement `register_host_memory()`: cudaHostRegister (tolerate already-registered) + spdk_mem_register at `components/gpu-services/src/lib.rs`
- [x] T061 [US7] Implement `unregister_host_memory()`: spdk_mem_unregister then cudaHostUnregister (reverse order) at `components/gpu-services/src/lib.rs`

**Checkpoint**: Pinned host buffers work for NVMe DMA and async GPU copies.

---

## Phase 10: User Story 8 - GPU-Direct P2P NVMe-to-GPU (Priority: P2)

**Goal**: True peer-to-peer NVMe-to-GPU DMA via GDRCopy BAR1 mapping, bypassing host memory

### Tests for User Story 8

- [x] T062 [P] [US8] Integration test: `test_nvme_to_gpu_p2p_gdrcopy` (end-to-end BAR1 P2P) at `components/gpu-services/tests/gpu_nvme_p2p.rs`
- [x] T063 [P] [US8] Integration test: `test_nvme_to_gpu_p2p_python_client` (cross-process IPC verification) at `components/gpu-services/tests/gpu_nvme_p2p.rs`
- [x] T064 [P] [US8] Integration test: `test_nvme_to_gpu_p2p_explicit_iommu` (decomposed registration path) at `components/gpu-services/tests/gpu_nvme_p2p.rs`

### Implementation for User Story 8

- [x] T065 [US8] Implement hand-written GDRCopy FFI bindings (gdr_open/close/pin_buffer/unpin_buffer/map/unmap) at `components/gpu-services/src/gdrcopy_ffi.rs`
- [x] T066 [US8] Implement `GdrMappingState` struct and `GDR_MAPPINGS` static for cleanup tracking at `components/gpu-services/src/dma.rs`
- [x] T067 [US8] Implement `spdk_unregister_gdr_unmap_and_close` free function (full P2P cleanup) at `components/gpu-services/src/dma.rs`
- [x] T068 [US8] Implement `create_spdk_dma_buffer_from_gpu_bar()`: gdr_open → pin → map → align → spdk_mem_register → DmaBuffer at `components/gpu-services/src/dma.rs`
- [x] T069 [US8] Implement `PhysMappingState` struct and `PHYS_MAPPINGS` static at `components/gpu-services/src/dma.rs`
- [x] T070 [US8] Implement `create_spdk_dma_buffer_from_phys()`: mmap → rte_extmem_register → rte_vfio_container_dma_map → DmaBuffer at `components/gpu-services/src/dma.rs`
- [x] T071 [US8] Implement `create_spdk_dma_buffer_from_bar_direct()`: identity IOVA mapping via DPDK APIs at `components/gpu-services/src/dma.rs`
- [x] T072 [P] [US8] Implement `spdk_unregister_and_cuda_free` free function for cudaMalloc'd buffers at `components/gpu-services/src/dma.rs`
- [x] T073 [P] [US8] Implement `create_spdk_dma_buffer_from_cuda_malloc()`: register + DmaBuffer for directly-allocated GPU memory at `components/gpu-services/src/dma.rs`
- [x] T074 [P] [US8] Implement `get_phys_addr()` helper: spdk_vtophys wrapper with error check at `components/gpu-services/src/dma.rs`
- [x] T075 [P] [US8] Implement `vfio_unmap_extmem_munmap` and `vfio_unmap_extmem_only` free functions at `components/gpu-services/src/dma.rs`

**Checkpoint**: NVMe reads land directly in GPU VRAM via GDRCopy BAR1 P2P DMA.

---

## Phase 11: User Story 8 (continued) - P2P Server Binary

**Goal**: Unix socket server accepting CUDA IPC handles with bounce/p2p/p2p-cold transfer modes

### Implementation for P2P Server

- [x] T076 [US8] Implement CLI argument parsing with clap (socket path, PCI address, mode, staging_size, chunk_size) at `components/gpu-services/src/bin/p2p_server.rs`
- [x] T077 [US8] Implement `initialize_stack()`: SPDK env init, NVMe device probe, CUDA init, component wiring at `components/gpu-services/src/bin/p2p_server.rs`
- [x] T078 [US8] Implement `create_gpu_staging()` and `create_chunk_pool()` for pre-pinned GPU buffers at `components/gpu-services/src/bin/p2p_server.rs`
- [x] T079 [US8] Implement `do_chunked_read()`: concurrent async NVMe reads via BatchSubmit at `components/gpu-services/src/bin/p2p_server.rs`
- [x] T080 [US8] Implement `handle_bounce()`: NVMe → host DMA → cudaMemcpy H2D → client GPU at `components/gpu-services/src/bin/p2p_server.rs`
- [x] T081 [US8] Implement `handle_p2p()`: NVMe → pre-pinned GPU staging → cudaMemcpy D2D → client GPU at `components/gpu-services/src/bin/p2p_server.rs`
- [x] T082 [US8] Implement `handle_p2p_cold()`: per-request GDRCopy pin/unpin (cold baseline) at `components/gpu-services/src/bin/p2p_server.rs`
- [x] T083 [US8] Implement main loop: Unix socket listener, signal handling (SIGINT/SIGTERM), client dispatch at `components/gpu-services/src/bin/p2p_server.rs`
- [x] T084 [US8] Implement `parse_client_payload()` and `open_ipc_handle()` helpers at `components/gpu-services/src/bin/p2p_server.rs`

**Checkpoint**: `gpu-p2p-server` binary serves NVMe→GPU transfers over Unix socket in all three modes.

---

## Phase 12: User Story 9 - Graceful Degradation (Priority: P3)

**Goal**: Compile and return clear errors without GPU hardware or feature flags

### Implementation for User Story 9

- [x] T085 [US9] Ensure all IGpuServices methods return "GPU support not compiled" when `gpu` feature disabled at `components/gpu-services/src/lib.rs`
- [x] T086 [US9] Ensure `shutdown()` returns Ok(()) even when not initialized at `components/gpu-services/src/lib.rs`
- [x] T087 [P] [US9] Ensure `GpuState` has a no-op default when `gpu` feature disabled at `components/gpu-services/src/lib.rs`
- [x] T088 [P] [US9] Verify crate compiles without CUDA/SPDK library presence when features disabled at `components/gpu-services/build.rs`

**Checkpoint**: `cargo build -p gpu-services` succeeds on any Linux machine without NVIDIA drivers.

---

## Phase 13: Benchmarks and Performance Validation

**Purpose**: Criterion-based DMA throughput benchmarks across transfer sizes and memory types

- [x] T089 [P] Implement `dma_transfer_benchmark.rs` with H2D/D2H pageable benchmarks (4K-64M) at `components/gpu-services/benches/dma_transfer_benchmark.rs`
- [x] T090 [P] Implement `dma_transfer_benchmark.rs` with H2D/D2H pinned benchmarks (4K-64M) at `components/gpu-services/benches/dma_transfer_benchmark.rs`
- [x] T091 [P] Implement `gpu_services_benchmark.rs` with component-level benchmarks at `components/gpu-services/benches/gpu_services_benchmark.rs`

**Checkpoint**: `cargo bench -p gpu-services --features gpu` runs all DMA throughput benchmarks.

---

## Phase 14: Polish and Cross-Cutting Concerns

**Purpose**: Documentation, lint compliance, and formal verification annotations

- [x] T092 [P] Add `// SAFETY:` justification comments to all unsafe blocks at all source files
- [x] T093 [P] Add doc comments with runnable examples to all public APIs at `components/interfaces/src/igpu_services.rs`
- [x] T094 [P] Ensure `cargo clippy -p gpu-services -- -D warnings` passes clean
- [x] T095 [P] Ensure `cargo doc -p gpu-services --no-deps` is warning-free
- [x] T096 [P] Add Creusot verification annotations (10 properties, 19 VCs) to interface definitions at `components/interfaces/src/igpu_services.rs`
- [x] T097 [P] Update component `CLAUDE.md` with active technologies and build instructions at `components/gpu-services/CLAUDE.md`
- [x] T098 [P] Create `README.md` with usage examples and hardware requirements at `components/gpu-services/README.md`

**Checkpoint**: All lints pass, docs build cleanly, safety comments present on all unsafe code.

---

## Dependencies & Execution Order

### Phase Dependencies

```
Phase 1 (Setup) ─────────────────────────────────────┐
Phase 2 (Foundational: CUDA FFI + Discovery) ────────┤
                                                      ├──► All User Stories
Phase 3 (US1: Device Discovery) ──────────────────────┤
Phase 4 (US2: IPC Handle) ───────────────────────────►│
Phase 5 (US3: Verify/Pin) ──────────────── depends on US2
Phase 6 (US4: SPDK Prepare) ─────────────── depends on US2 + US3
Phase 7 (US5: Sync DMA) ────────────────── depends on Phase 2
Phase 8 (US6: Async DMA) ───────────────── depends on Phase 2
Phase 9 (US7: Pinned Host) ─────────────── depends on Phase 2
Phase 10 (US8: P2P GDRCopy) ────────────── depends on US4
Phase 11 (US8: P2P Server) ─────────────── depends on Phase 10
Phase 12 (US9: Degradation) ────────────── independent (any time)
Phase 13 (Benchmarks) ──────────────────── depends on Phase 2
Phase 14 (Polish) ──────────────────────── depends on all above
```

### Critical Path

```
Setup → CUDA FFI → Device Discovery → IPC Decode → Memory Verify/Pin
    → SPDK DMA Buffer → GDRCopy P2P → P2P Server → Polish
```

### Parallel Opportunities

- US1 (Discovery), US5 (Sync DMA), US6 (Async DMA), US7 (Pinned Host), US9 (Degradation) are all parallelizable after Phase 2
- All benchmark tasks (T089-T091) are independent of each other
- All polish tasks (T092-T098) are independent of each other
- P2P integration tests (T062-T064) can run in parallel

---

## Notes

- All tasks are marked COMPLETE as this is a backfill of existing implementation
- Hardware-dependent tests self-skip when prerequisites are unavailable
- The P2P server uses `atexit(_exit)` to avoid SPDK teardown crashes in test contexts
- GDRCopy requires memory allocated by the current process (not IPC-opened memory)
- Feature gate hierarchy: `p2p` implies `gpu` + `spdk`; `spdk` alone does NOT imply `gpu`
