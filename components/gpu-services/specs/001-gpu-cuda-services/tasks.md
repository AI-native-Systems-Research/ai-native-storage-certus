# Tasks: GPU CUDA Services

**Input**: Design documents from `/specs/001-gpu-cuda-services/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Tests ARE required per constitution (Principle IV: Comprehensive Unit Testing, Principle V: Rust Documentation Tests, Principle VI: Criterion Performance Benchmarks).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup

**Purpose**: Project initialization, feature gate setup, and CUDA FFI foundation

- [x] T001 Add `gpu` feature gate to `components/gpu-services/v0/Cargo.toml` with dependencies (base64, libc)
- [x] T002 [P] Create CUDA FFI bindings module at `components/gpu-services/v0/src/cuda_ffi.rs` with extern declarations for cudaGetDeviceCount, cudaGetDeviceProperties, cudaIpcOpenMemHandle, cudaIpcCloseMemHandle, cudaPointerGetAttributes, cudaHostRegister, cudaHostUnregister, cudaSetDevice, cudaDeviceSynchronize, cudaFree (all cfg(feature="gpu"))
- [x] T003 [P] Add `GpuDeviceInfo` struct to `components/interfaces/src/igpu_services.rs`
- [x] T004 [P] Expand `IGpuServices` trait in `components/interfaces/src/igpu_services.rs` with all methods: get_devices, deserialize_ipc_handle, verify_memory, pin_memory, unpin_memory, create_dma_buffer (per contracts/igpu_services.md)
- [x] T005 [P] Add `gpu` feature to `components/interfaces/Cargo.toml` gating DmaBuffer/IpcHandle re-exports needed by IGpuServices methods
- [x] T006 Create internal module stubs: `components/gpu-services/v0/src/device.rs`, `components/gpu-services/v0/src/ipc.rs`, `components/gpu-services/v0/src/memory.rs`, `components/gpu-services/v0/src/dma.rs` (all cfg(feature="gpu"))

**Checkpoint**: Project compiles with `cargo build -p gpu-services --features gpu` (methods may return `unimplemented!()` stubs)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core infrastructure that MUST be complete before ANY user story implementation

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T007 Implement component state management in `components/gpu-services/v0/src/lib.rs`: add internal state struct holding Vec<GpuDeviceInfo>, initialization flag, and tracked IPC handles (behind Mutex for thread safety)
- [x] T008 [P] Implement stub IGpuServices trait methods in `components/gpu-services/v0/src/lib.rs` that delegate to internal modules (device, ipc, memory, dma) based on initialization state
- [x] T009 [P] Add CUDA error code translation helper in `components/gpu-services/v0/src/cuda_ffi.rs`: convert cudaError_t enum values to descriptive String errors
- [x] T010 [P] Create benchmark harness at `components/gpu-services/v0/benches/gpu_services_benchmark.rs` with empty Criterion groups for each performance-sensitive operation (feature-gated)
- [x] T011 Add doc comments with runnable examples to all IGpuServices trait methods in `components/interfaces/src/igpu_services.rs` (per constitution Principle V)

**Checkpoint**: Foundation ready — all interface methods exist with delegation stubs, benchmark harness compiles, doc tests pass

---

## Phase 3: User Story 1 — Initialize CUDA and Discover GPU Hardware (Priority: P1)

**Goal**: Initialize CUDA libraries and enumerate GPUs with compute capability 7.0+

**Independent Test**: Call `initialize()` and verify `get_devices()` returns correct GPU information

### Tests for User Story 1

- [x] T012 [P] [US1] Write unit test `test_initialize_success` in `components/gpu-services/v0/src/lib.rs`: verify initialize() returns Ok on system with GPU
- [x] T013 [P] [US1] Write unit test `test_initialize_idempotent` in `components/gpu-services/v0/src/lib.rs`: verify calling initialize() twice succeeds without error
- [x] T014 [P] [US1] Write unit test `test_get_devices_returns_info` in `components/gpu-services/v0/src/lib.rs`: verify get_devices() returns non-empty vec with valid fields after initialize
- [x] T015 [P] [US1] Write unit test `test_get_devices_before_init_fails` in `components/gpu-services/v0/src/lib.rs`: verify get_devices() returns error before initialize
- [x] T016 [P] [US1] Write unit test `test_devices_filter_compute_capability` in `components/gpu-services/v0/src/lib.rs`: verify only GPUs with compute >= 7.0 are returned
- [x] T017 [P] [US1] Write unit test `test_shutdown_releases_state` in `components/gpu-services/v0/src/lib.rs`: verify shutdown clears device list and allows re-initialization

### Implementation for User Story 1

- [x] T018 [US1] Implement `discover_devices()` in `components/gpu-services/v0/src/device.rs`: call cudaGetDeviceCount + cudaGetDeviceProperties for each device, filter compute >= 7.0, return Vec<GpuDeviceInfo>
- [x] T019 [US1] Implement `initialize()` in `components/gpu-services/v0/src/lib.rs`: call cuda_ffi init, run discover_devices, store results in component state
- [x] T020 [US1] Implement `get_devices()` in `components/gpu-services/v0/src/lib.rs`: return cloned device list from state (error if not initialized)
- [x] T021 [US1] Implement `shutdown()` in `components/gpu-services/v0/src/lib.rs`: close any open IPC handles, clear device list, reset initialization flag
- [x] T022 [US1] Add Criterion benchmark `bench_initialize` in `components/gpu-services/v0/benches/gpu_services_benchmark.rs`: measure initialize() latency (target <5s)
- [x] T023 [US1] Add Criterion benchmark `bench_get_devices` in `components/gpu-services/v0/benches/gpu_services_benchmark.rs`: measure get_devices() latency (target <1ms cached)

**Checkpoint**: `cargo test -p gpu-services --features gpu` passes US1 tests; `cargo bench -p gpu-services --features gpu` runs initialize/get_devices benchmarks

---

## Phase 4: User Story 2 — Deserialize Python IPC Handle (Priority: P2)

**Goal**: Decode base64-encoded CUDA IPC handle + size from Python client into usable IpcHandle

**Independent Test**: Provide known base64 payloads and verify deserialization produces correct handle/size

### Tests for User Story 2

- [x] T024 [P] [US2] Write unit test `test_deserialize_valid_payload` in `components/gpu-services/v0/src/ipc.rs`: verify correct base64 72-byte payload deserializes to valid IpcHandle
- [x] T025 [P] [US2] Write unit test `test_deserialize_invalid_base64` in `components/gpu-services/v0/src/ipc.rs`: verify malformed base64 returns error
- [x] T026 [P] [US2] Write unit test `test_deserialize_wrong_size` in `components/gpu-services/v0/src/ipc.rs`: verify payload != 72 decoded bytes returns error
- [x] T027 [P] [US2] Write unit test `test_deserialize_before_init_fails` in `components/gpu-services/v0/src/lib.rs`: verify deserialize_ipc_handle returns error if not initialized

### Implementation for User Story 2

- [x] T028 [US2] Implement `decode_ipc_payload(base64_str) -> Result<(CudaIpcHandle, u64), String>` in `components/gpu-services/v0/src/ipc.rs`: base64 decode, split at byte 64, parse handle bytes + LE u64 size
- [x] T029 [US2] Implement `open_ipc_handle(raw_handle) -> Result<*mut u8, String>` in `components/gpu-services/v0/src/ipc.rs`: call cudaIpcOpenMemHandle, return device pointer
- [x] T030 [US2] Implement `deserialize_ipc_handle()` in `components/gpu-services/v0/src/lib.rs`: delegate to ipc module, construct IpcHandle{address, size}, track in component state
- [x] T031 [US2] Add Criterion benchmark `bench_deserialize_ipc_handle` in `components/gpu-services/v0/benches/gpu_services_benchmark.rs`: measure deserialization latency (target <1ms)

**Checkpoint**: US2 tests pass; deserialization benchmark runs (target <1ms)

---

## Phase 5: User Story 3 — Verify GPU Memory Contiguity and Pin Status (Priority: P3)

**Goal**: Validate that IPC handle memory is device-type, contiguous, and suitable for DMA

**Independent Test**: Verify memory check passes for valid device pointers and fails for invalid ones

### Tests for User Story 3

- [x] T032 [P] [US3] Write unit test `test_verify_valid_device_memory` in `components/gpu-services/v0/src/memory.rs`: verify check passes for cudaMalloc'd memory opened via IPC
- [x] T033 [P] [US3] Write unit test `test_verify_null_handle_fails` in `components/gpu-services/v0/src/memory.rs`: verify null address returns error
- [x] T034 [P] [US3] Write unit test `test_verify_before_init_fails` in `components/gpu-services/v0/src/lib.rs`: verify verify_memory returns error if not initialized

### Implementation for User Story 3

- [x] T035 [US3] Implement `check_memory_attributes(ptr) -> Result<(), String>` in `components/gpu-services/v0/src/memory.rs`: call cudaPointerGetAttributes, verify memoryType == cudaMemoryTypeDevice
- [x] T036 [US3] Implement `verify_memory()` in `components/gpu-services/v0/src/lib.rs`: delegate to memory module, validate handle is tracked
- [x] T037 [US3] Add Criterion benchmark `bench_verify_memory` in `components/gpu-services/v0/benches/gpu_services_benchmark.rs`: measure verification latency (target <10ms)

**Checkpoint**: US3 tests pass; verification benchmark runs

---

## Phase 6: User Story 4 — Pin and Unpin GPU Memory (Priority: P4)

**Goal**: Provide pin/unpin operations for GPU memory lifecycle management

**Independent Test**: Pin memory, verify pin status, unpin, verify released

### Tests for User Story 4

- [x] T038 [P] [US4] Write unit test `test_pin_memory_success` in `components/gpu-services/v0/src/memory.rs`: verify pin returns Ok for valid device pointer
- [x] T039 [P] [US4] Write unit test `test_pin_idempotent` in `components/gpu-services/v0/src/memory.rs`: verify pinning already-pinned memory succeeds
- [x] T040 [P] [US4] Write unit test `test_unpin_memory_success` in `components/gpu-services/v0/src/memory.rs`: verify unpin releases pinned memory
- [x] T041 [P] [US4] Write unit test `test_unpin_not_pinned_fails` in `components/gpu-services/v0/src/memory.rs`: verify unpinning non-pinned memory returns error

### Implementation for User Story 4

- [x] T042 [US4] Implement `pin_gpu_memory(ptr, size) -> Result<(), String>` in `components/gpu-services/v0/src/memory.rs`: call cudaHostRegister with appropriate flags, track pinned state
- [x] T043 [US4] Implement `unpin_gpu_memory(ptr, size) -> Result<(), String>` in `components/gpu-services/v0/src/memory.rs`: call cudaHostUnregister, remove from tracked state
- [x] T044 [US4] Implement `pin_memory()` and `unpin_memory()` in `components/gpu-services/v0/src/lib.rs`: delegate to memory module, validate handle tracking
- [x] T045 [US4] Add Criterion benchmark `bench_pin_unpin` in `components/gpu-services/v0/benches/gpu_services_benchmark.rs`: measure pin + unpin round-trip latency

**Checkpoint**: US4 tests pass; pin/unpin benchmark runs

---

## Phase 7: User Story 5 — Create DMA Buffer from IPC Handle (Priority: P5)

**Goal**: Create a DmaBuffer backed by GPU memory from a verified, pinned IPC handle

**Independent Test**: Deserialize handle, verify, pin, create DmaBuffer, check properties

### Tests for User Story 5

- [x] T046 [P] [US5] Write unit test `test_create_dma_buffer_success` in `components/gpu-services/v0/src/dma.rs`: verify DmaBuffer created with correct size and non-null ptr
- [x] T047 [P] [US5] Write unit test `test_create_dma_buffer_unverified_fails` in `components/gpu-services/v0/src/dma.rs`: verify creation fails if handle not verified
- [x] T048 [P] [US5] Write unit test `test_create_dma_buffer_unpinned_fails` in `components/gpu-services/v0/src/dma.rs`: verify creation fails if handle not pinned
- [x] T049 [P] [US5] Write unit test `test_dma_buffer_drop_closes_ipc` in `components/gpu-services/v0/src/dma.rs`: verify dropping DmaBuffer calls cudaIpcCloseMemHandle

### Implementation for User Story 5

- [x] T050 [US5] Implement `create_gpu_dma_buffer(handle, size) -> Result<DmaBuffer, String>` in `components/gpu-services/v0/src/dma.rs`: create extern "C" free_fn wrapping cudaIpcCloseMemHandle, call DmaBuffer::from_raw with GPU ptr + free_fn
- [x] T051 [US5] Implement `create_dma_buffer()` in `components/gpu-services/v0/src/lib.rs`: validate handle is verified + pinned, delegate to dma module, consume handle from tracking
- [x] T052 [US5] Add Criterion benchmark `bench_create_dma_buffer` in `components/gpu-services/v0/benches/gpu_services_benchmark.rs`: measure DMA buffer creation latency (target <50ms)

**Checkpoint**: US5 tests pass; full pipeline (init → deserialize → verify → pin → create DMA) works end-to-end in a single test

---

## Phase 8: User Story 6 — Python-to-Rust IPC Handle Handoff Demo (Priority: P6)

**Goal**: End-to-end demo app showing Python client handing off GPU IPC handle to Rust server via Unix domain socket

**Independent Test**: Run both apps together; Python allocates GPU memory, Rust receives handle and performs DMA from SPDK CPU memory

### Tests for User Story 6

- [x] T053 [P] [US6] Write integration test `test_unix_socket_protocol` in `components/gpu-services/v0/tests/integration.rs`: simulate client sending valid payload over Unix socket to server handler function

### Implementation for User Story 6

- [x] T054 [US6] Create `apps/gpu-handle-test-server/Cargo.toml` with dependencies on gpu-services, interfaces, component-framework (feature gpu)
- [x] T055 [US6] Implement Unix domain socket server in `apps/gpu-handle-test-server/src/main.rs`: listen on socket path, accept connection, read length-prefixed base64 payload, deserialize IPC handle, verify, pin, create DMA buffer, send ACK/NACK response
- [x] T056 [US6] Create `apps/gpu-handle-test-client/requirements.txt` with cupy dependency
- [x] T057 [US6] Implement Python client in `apps/gpu-handle-test-client/client.py`: allocate GPU memory with cupy, get IPC handle, base64 encode (handle + LE size), connect to Unix socket, send length-prefixed payload, await ACK
- [x] T058 [US6] Add README for demo apps at `apps/gpu-handle-test-server/README.md` documenting build, run, and expected output

**Checkpoint**: Full end-to-end demo works: Python allocates → serializes → sends → Rust deserializes → verifies → creates DMA buffer → ACKs

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Final quality gates, documentation, and CI integration

- [x] T059 [P] Run `cargo clippy -p gpu-services --features gpu -- -D warnings` and fix all warnings
- [x] T060 [P] Run `cargo fmt --check` on all modified files and fix formatting
- [x] T061 [P] Run `cargo doc -p gpu-services --no-deps --features gpu` and fix all doc warnings
- [x] T062 Ensure `#![deny(missing_docs)]` is set in `components/gpu-services/v0/src/lib.rs` and all public items have doc comments
- [x] T063 [P] Verify all `unsafe` blocks have `// SAFETY:` comments in `components/gpu-services/v0/src/cuda_ffi.rs` and `components/gpu-services/v0/src/dma.rs`
- [x] T064 Run full test suite: `cargo test -p gpu-services --features gpu -- --test-threads 1` and verify all pass
- [x] T065 Run full benchmark suite: `cargo bench -p gpu-services --features gpu` and verify no errors
- [x] T066 Verify interface gate: confirm no `pub fn` exists outside IGpuServices trait in the gpu-services crate

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational — first story to implement
- **US2 (Phase 4)**: Depends on Foundational; uses IpcHandle type from US1 interface expansion
- **US3 (Phase 5)**: Depends on US2 (needs deserialized IPC handle to verify)
- **US4 (Phase 6)**: Depends on US3 (pin after verify)
- **US5 (Phase 7)**: Depends on US3 + US4 (needs verified + pinned handle)
- **US6 (Phase 8)**: Depends on US1–US5 (full pipeline)
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Dependencies

- **US1**: Can start after Foundational — no dependencies on other stories
- **US2**: Can start after Foundational — uses IGpuServices interface from US1 setup but is independently testable
- **US3**: Depends on US2 (needs IPC handle to verify)
- **US4**: Depends on US3 (pin after verify in the pipeline)
- **US5**: Depends on US3 + US4 (needs verified AND pinned handle)
- **US6**: Depends on US1–US5 (end-to-end demo)

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- FFI bindings before high-level logic
- Internal module functions before IGpuServices delegation
- Benchmarks after implementation (need working code to measure)

### Parallel Opportunities

- All Setup tasks T002–T006 can run in parallel (different files)
- Within each user story, all test tasks marked [P] can run in parallel
- US1 and US2 can be developed concurrently (independent after Foundational)
- All Polish tasks marked [P] can run in parallel

---

## Parallel Examples

### Setup Phase

```bash
# All can run in parallel (different files):
T002: Create cuda_ffi.rs
T003: Add GpuDeviceInfo to interfaces
T004: Expand IGpuServices trait
T005: Add gpu feature to interfaces Cargo.toml
T006: Create internal module stubs
```

### User Story 1 Tests

```bash
# All test tasks in parallel (same file but independent test functions):
T012: test_initialize_success
T013: test_initialize_idempotent
T014: test_get_devices_returns_info
T015: test_get_devices_before_init_fails
T016: test_devices_filter_compute_capability
T017: test_shutdown_releases_state
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational
3. Complete Phase 3: User Story 1 (Initialize + Discover)
4. **STOP and VALIDATE**: `cargo test -p gpu-services --features gpu` passes
5. Component can discover GPUs — useful standalone

### Incremental Delivery

1. Setup + Foundational → Build compiles
2. Add US1 → Initialize and discover GPUs
3. Add US2 → Deserialize IPC handles from Python
4. Add US3 → Verify GPU memory safety
5. Add US4 → Pin/unpin memory lifecycle
6. Add US5 → Create DMA buffers (full pipeline)
7. Add US6 → Demo apps proving end-to-end
8. Polish → All quality gates pass

### Sequential Strategy (Recommended)

This feature has a natural pipeline dependency (init → deserialize → verify → pin → DMA buffer), so sequential execution P1→P6 is the most efficient path. US1 and US2 can overlap slightly but US3–US6 are strictly ordered.
