# Tasks: SPDK NVMe Block Device Component

**Feature Branch**: `001-block-device-spdk-nvme`
**Created**: 2026-07-08
**Status**: Backfilled (all tasks complete)
**Spec**: [spec.md](spec.md) | **Plan**: [plan.md](plan.md)

> This task list was generated from existing implementation via backfill.
> All tasks are marked complete. Use for reference and future maintenance.

---

## Phase 1: Foundation (Component Shell + FFI Bindings)

### Task 1.1: Define Component Skeleton
- **Status**: Done
- **File**: `src/lib.rs`
- **Description**: Use `define_component!` macro to generate `BlockDeviceSpdkNvmeComponent` with version "0.2.0", provided interfaces `[IBlockDevice, IBlockDeviceAdmin]`, and receptacles `{spdk_env: ISPDKEnv, logger: ILogger}`.
- **Fields**: `pci_address: RwLock<Option<PciAddress>>`, `actor_cpu: Mutex<Option<usize>>`, `controller_info: RwLock<Option<ControllerSnapshot>>`, `actor_handle: Mutex<Option<ActorHandle<ControlMessage>>>`, `controller_park: Arc<Mutex<Option<NvmeController>>>`, `next_client_id: AtomicU64`, `telemetry_stats: Mutex<Option<Arc<dyn Any + Send + Sync>>>`.
- **Acceptance**: `cargo build` succeeds; `IUnknown::version()` returns "0.2.0"; `provided_interfaces()` includes `IBlockDevice`; `receptacles()` includes `spdk_env` and `logger`.
- **Refs**: FR-001, FR-002, FR-003, FR-025

### Task 1.2: Define Internal Message Types
- **Status**: Done
- **File**: `src/command.rs`
- **Description**: Define `ClientSession` (id, ingress_rx, callback_tx) and `ControlMessage` enum (`ConnectClient { session }`, `DisconnectClient { client_id }`).
- **Acceptance**: Types compile; `ControlMessage` is `Send`.

### Task 1.3: Implement TSC Clock
- **Status**: Done
- **File**: `src/tsc.rs`
- **Description**: Implement `TscClock` with 2ms calibration window against `clock_gettime`, `rdtsc()` helper, `now()`, `ticks_to_ns()` (fixed-point Q32 multiply), `deadline_from_ms()`, `has_elapsed()`. Fallback to `Instant::now()` on non-x86_64.
- **Acceptance**: Unit tests pass: `rdtsc_monotonic`, `calibration_reasonable` (500MHz-10GHz), `ticks_to_ns_round_trip` (10ms +/- 5ms), `deadline_from_ms_works`.
- **Refs**: NFR-005, NFR-013

### Task 1.4: Implement QueuePair and QueuePairPool
- **Status**: Done
- **File**: `src/qpair.rs`
- **Description**: Implement `QueuePair` (depth tracking, submit/complete, `process_completions()` via SPDK FFI, `new_detached()` for tests) and `QueuePairPool` (standard depths [4, 16, 64, 256], `allocate()` from SPDK controller, `from_detached()` for tests, `select_index()` shallowest-fit heuristic with most-available fallback, `select_qpair()`, `deallocate_all()`). Set `io_queue_requests = depth * 4` in qpair opts.
- **Acceptance**: Unit tests pass: detached state, submit/complete tracking, pool selection at varying batch sizes, pressure-based fallback. `allocate()` succeeds on real hardware.
- **Refs**: NFR-006, NFR-007, NFR-012

### Task 1.5: Implement NvmeController
- **Status**: Done
- **File**: `src/controller.rs`
- **Description**: Implement safe wrapper around `*mut spdk_nvme_ctrlr`: `attach()` (null check, get_default_ctrlr_opts, discover_namespaces, allocate QueuePairPool), `discover_namespaces()` (iterate ns_ids, check active, get sectors/size), accessor methods (sector_size, num_sectors, max_queue_depth, num_io_queues, max_transfer_size, numa_node, version), `refresh_namespaces()`, Drop (deallocate_all qpairs then `spdk_nvme_detach`). `NvmeVersion` Display impl. `NvmeNamespaceInfo` with `capacity_bytes()`.
- **Acceptance**: Unit tests pass: NvmeVersion display/equality, NvmeNamespaceInfo capacity/clone. Integration: attach to real controller, discover namespaces.
- **Refs**: FR-010, NFR-014

---

## Phase 2: Actor Core (Lifecycle + Client Management)

### Task 2.1: Implement BlockDeviceHandler Skeleton
- **Status**: Done
- **File**: `src/actor.rs`
- **Description**: Implement `BlockDeviceHandler` struct with fields: controller (Option), controller_park (Arc), clients (Vec<ClientState>), next_handle, async_completions, completion_scratch, context_pool (capacity 340), timeout_scratch, telemetry (feature-gated), last_timeout_check, poll_start_idx, tsc, logger. Constructors: `new()` and `with_telemetry()`.
- **Acceptance**: Compiles with and without `telemetry` feature.
- **Refs**: NFR-004

### Task 2.2: Implement ActorHandler Trait
- **Status**: Done
- **File**: `src/actor.rs`
- **Description**: Implement `ActorHandler<ControlMessage>` for `BlockDeviceHandler`: `handle()` (process ConnectClient/DisconnectClient, then poll_clients + check_timeouts), `on_idle()` (poll_clients, throttled check_timeouts ~1ms, return true while clients connected), `on_start()` (log), `on_stop()` (drain qpairs 5s, abort pending, clear clients, park controller).
- **Acceptance**: Actor can be activated/deactivated. `on_stop` parks controller correctly. Spinning behavior: on_idle returns true when clients exist.
- **Refs**: FR-019, FR-020, FR-026, NFR-008

### Task 2.3: Implement connect_client()
- **Status**: Done
- **File**: `src/lib.rs`
- **Description**: Implement `IBlockDevice::connect_client()`: check actor_handle exists (else NotInitialized), allocate client_id (AtomicU64), create two `SpscChannel` of capacity 256, split into tx/rx pairs, send `ControlMessage::ConnectClient` to actor, return `ClientChannels { command_tx, completion_rx }`.
- **Acceptance**: Unit test: connect_client returns NotInitialized without initialize(). Integration: connect_client succeeds after initialize(), returned channels are functional.
- **Refs**: FR-004, FR-024, NFR-002

### Task 2.4: Implement initialize()
- **Status**: Done
- **File**: `src/lib.rs`
- **Description**: Check spdk_env receptacle is connected (else NotInitialized). Call `probe_controller()` (unsafe: build SPDK transport ID from PCI address, `spdk_nvme_probe` with probe_cb/attach_cb). Attach via `NvmeController::attach()`. Snapshot controller info. Create BlockDeviceHandler (with or without telemetry). Create Actor, set CPU affinity (explicit or NUMA-derived), activate, store handle.
- **Acceptance**: Returns NotInitialized if spdk_env not wired. Returns ProbeFailure on bad PCI. Succeeds with real hardware. Actor starts spinning.
- **Refs**: FR-023, NFR-001

### Task 2.5: Implement IBlockDeviceAdmin
- **Status**: Done
- **File**: `src/lib.rs`
- **Description**: Implement `set_pci_address()`, `set_actor_cpu()`, `initialize()` (delegates), `signal_stop()` (handle.signal_stop), `shutdown()` (take handle, deactivate), `detach_controller()` (take from parking slot, Drop detaches).
- **Acceptance**: Shutdown test passes: actor deactivates cleanly. Detach releases controller.
- **Refs**: FR-002

---

## Phase 3: Synchronous IO

### Task 3.1: Implement Synchronous Read
- **Status**: Done
- **File**: `src/actor.rs` (`do_sync_read`, `poll_sync_completion`)
- **Description**: Validate ns_id and LBA range. Get ns_ptr via `spdk_nvme_ctrlr_get_ns`. Create `SyncCompletionCtx` (AtomicBool done, AtomicU16 sct/sc). Select qpair (batch=1). Submit `spdk_nvme_ns_cmd_read()` with `sync_completion_cb`. Spin calling `qp.process_completions(0)` until done. Check sct/sc for errors. Send `Completion::ReadDone`.
- **Acceptance**: Integration test: write then read-back verifies data integrity. Error case: LBA out of range returns LbaOutOfRange.
- **Refs**: FR-005, FR-010, FR-011

### Task 3.2: Implement Synchronous Write
- **Status**: Done
- **File**: `src/actor.rs` (`do_sync_write`)
- **Description**: Same pattern as sync read but with `spdk_nvme_ns_cmd_write`. `Arc<DmaBuffer>` (immutable after fill). Send `Completion::WriteDone`.
- **Acceptance**: Integration test: write succeeds, data readable afterward. Error on bad LBA.
- **Refs**: FR-006, FR-010, FR-011

### Task 3.3: Implement Write Zeros
- **Status**: Done
- **File**: `src/actor.rs` (`do_write_zeros`)
- **Description**: Validate ns/LBA. Submit `spdk_nvme_ns_cmd_write_zeroes` synchronously (same poll pattern). Send `Completion::WriteZerosDone`.
- **Acceptance**: Integration test: write zeros, then read verifies all-zero buffer.
- **Refs**: FR-009

---

## Phase 4: Asynchronous IO

### Task 4.1: Implement ContextPool
- **Status**: Done
- **File**: `src/actor.rs`
- **Description**: Slab allocator for `AsyncIoContext`. `acquire()` pops from pool or allocates new Box. `release()` pushes back. Pre-capacity 340 (sum of standard depths + headroom). Single-threaded (actor only).
- **Acceptance**: Acquire/release cycle works without leaks. No heap allocation after warmup.
- **Refs**: NFR-004

### Task 4.2: Implement Async Completion Callback
- **Status**: Done
- **File**: `src/actor.rs` (`async_completion_cb`)
- **Description**: `unsafe extern "C"` callback: reconstruct `Box<AsyncIoContext>` from raw pointer, extract NVMe status (sct/sc), compute telemetry latency if feature enabled, push `AsyncCompletionEntry` to completions Vec, release context back to pool.
- **Acceptance**: Callback fires correctly during process_completions(). Context pool stays balanced.
- **Refs**: NFR-014

### Task 4.3: Implement Asynchronous Read
- **Status**: Done
- **File**: `src/actor.rs` (within `dispatch_command`, `Command::ReadAsync` arm)
- **Description**: Validate ns/LBA (extract buf_ptr from Mutex in single lock). Select qpair via `select_index(pending_ops.len() + 1)`. Acquire context, fill fields. Submit `spdk_nvme_ns_cmd_read` with `async_completion_cb`. On `-ENOMEM`: retry loop polling completions up to `min(timeout_ms, 1000ms)`. On success: `qp.submit()`, insert `PendingOp` with TSC deadline. On final failure: release context, remove pending, send ReadDone with error.
- **Acceptance**: Integration test: async read completes with correct data. ENOMEM retry works under load. Timeout fires if deadline exceeded.
- **Refs**: FR-007, FR-022

### Task 4.4: Implement Asynchronous Write
- **Status**: Done
- **File**: `src/actor.rs` (within `dispatch_command`, `Command::WriteAsync` arm)
- **Description**: Same pattern as async read with `spdk_nvme_ns_cmd_write`. `Arc<DmaBuffer>` held in PendingOp.write_buf to keep DMA memory alive. Send `Completion::WriteDone` on completion.
- **Acceptance**: Integration test: async write then sync read verifies data. Concurrent async writes do not corrupt.
- **Refs**: FR-008, FR-022

### Task 4.5: Implement Timeout Checking
- **Status**: Done
- **File**: `src/actor.rs` (`check_timeouts`)
- **Description**: Called from `on_idle()` when `tsc.now() >= deadline_from_ms(last_timeout_check, 1)`. Iterates all clients' pending_ops, removes expired ops (deadline <= now), sends `Completion::Timeout { handle }`.
- **Acceptance**: Integration test: async read with very short timeout receives Timeout completion.
- **Refs**: FR-021

### Task 4.6: Implement Completion Drain
- **Status**: Done
- **File**: `src/actor.rs` (within `poll_clients`)
- **Description**: After polling all qpairs, swap `async_completions` into `completion_scratch`. Drain scratch: match client_id, remove from pending_ops (silently discard if already removed by abort/timeout), record telemetry, build Completion, send on callback_tx.
- **Acceptance**: Completions reach correct client. Double-delivery impossible (pending_ops.remove gate).

---

## Phase 5: Advanced Operations

### Task 5.1: Implement Batch Submission
- **Status**: Done
- **File**: `src/actor.rs` (within `dispatch_command`, `Command::BatchSubmit` arm)
- **Description**: Select a single qpair for entire batch via `select_index(ops.len())`. Recursively dispatch each sub-command with `qp_idx_override = Some(batch_qp_idx)`.
- **Acceptance**: Integration test: batch of mixed reads/writes all route to same qpair. Individual completions arrive.
- **Refs**: FR-012

### Task 5.2: Implement Operation Abort
- **Status**: Done
- **File**: `src/actor.rs` (within `dispatch_command`, `Command::AbortOp` arm)
- **Description**: Remove handle from pending_ops (if present). Send `Completion::AbortAck { handle }` regardless (idempotent).
- **Acceptance**: Abort of pending op prevents future timeout/completion. Abort of non-existent handle still acks.
- **Refs**: FR-013

### Task 5.3: Implement Controller Reset
- **Status**: Done
- **File**: `src/actor.rs` (`handle_controller_reset`)
- **Description**: Intercepted in `poll_clients()` before `dispatch_command()` (needs access to ALL clients). Cancel all pending ops across ALL clients with `Completion::Error { Aborted }`. Call `spdk_nvme_ctrlr_reset()`. Refresh namespaces. Send `Completion::ResetDone` to requesting client.
- **Acceptance**: Integration test: pending ops across multiple clients all get aborted. Requesting client gets ResetDone. Controller functional after reset.
- **Refs**: FR-018

### Task 5.4: Implement Namespace Probe
- **Status**: Done
- **File**: `src/actor.rs` + `src/namespace.rs`
- **Description**: On `Command::NsProbe`, convert controller's `namespaces` Vec via `to_namespace_info_list()`. Send `Completion::NsProbeResult { namespaces }`.
- **Acceptance**: Integration test: NsProbe returns correct namespace list matching controller state.
- **Refs**: FR-014

### Task 5.5: Implement Namespace Create
- **Status**: Done
- **File**: `src/namespace.rs` (`create`)
- **Description**: Compute effective size (use `unallocated_sectors()` if size_sectors==0). Create namespace via `spdk_nvme_ctrlr_create_ns`. Attach via `spdk_nvme_ctrlr_attach_ns`. Refresh namespace list. Send `Completion::NsCreated { ns_id }`.
- **Acceptance**: Integration test: create namespace, verify in subsequent NsProbe. Handles zero-size (use all remaining). Rolls back on attach failure.
- **Refs**: FR-015

### Task 5.6: Implement Namespace Format
- **Status**: Done
- **File**: `src/namespace.rs` (`format`)
- **Description**: Call `spdk_nvme_ctrlr_format` with lbaf. Issue controller reset to refresh identify data. Refresh namespaces. Send `Completion::NsFormatted { ns_id }`.
- **Acceptance**: Integration test: format namespace, verify new sector size via NsProbe.
- **Refs**: FR-016

### Task 5.7: Implement Namespace Delete
- **Status**: Done
- **File**: `src/namespace.rs` (`delete`)
- **Description**: Call `spdk_nvme_ctrlr_delete_ns`. Refresh namespace list. Send `Completion::NsDeleted { ns_id }`.
- **Acceptance**: Integration test: delete namespace, verify gone in subsequent NsProbe.
- **Refs**: FR-017

---

## Phase 6: Device Introspection

### Task 6.1: Implement Device Info Methods
- **Status**: Done
- **File**: `src/lib.rs`
- **Description**: Implement `IBlockDevice` methods (`sector_size`, `num_sectors`, `max_queue_depth`, `num_io_queues`, `max_transfer_size`, `block_size`, `numa_node`, `nvme_version`) reading from `ControllerSnapshot`. Return safe defaults when not initialized (0, 512, -1, "unknown").
- **Acceptance**: Unit test: defaults returned before init. Integration: correct values after init.
- **Refs**: FR-001

---

## Phase 7: Telemetry

### Task 7.1: Implement TelemetryStats
- **Status**: Done
- **File**: `src/telemetry.rs`
- **Description**: Feature-gated `TelemetryStats`: atomic counters for total_ops, min_latency_ns (CAS loop), max_latency_ns (CAS loop), sum_latency_ns, total_bytes. `record()` updates all atomics with Relaxed ordering. `snapshot()` computes mean_latency, mean_throughput_mbps.
- **Acceptance**: Unit tests: single/multiple record, empty snapshot, min/max tracking.
- **Refs**: NFR-010, NFR-011

### Task 7.2: Wire Telemetry into Actor
- **Status**: Done
- **File**: `src/actor.rs`, `src/lib.rs`
- **Description**: `with_telemetry()` constructor accepts shared `Arc<TelemetryStats>`. Sync path: record after successful completion. Async path: `async_completion_cb` computes latency from TSC delta, completion drain calls `telemetry.record()`. Component stores type-erased Arc in `telemetry_stats` field. `IBlockDevice::telemetry()` downcasts and calls `snapshot()`.
- **Acceptance**: Integration test (with feature): telemetry values accurate within 5% of independent measurement. Without feature: returns FeatureNotEnabled.
- **Refs**: FR-001, NFR-010

---

## Phase 8: Graceful Shutdown

### Task 8.1: Implement signal_stop + shutdown + detach
- **Status**: Done
- **File**: `src/lib.rs`
- **Description**: `signal_stop()`: close actor command channel. `shutdown()`: take actor handle, deactivate (joins thread). `detach_controller()`: take from parking slot (Drop runs spdk_nvme_detach, freeing qpairs first).
- **Acceptance**: Shutdown test: actor deactivates cleanly. No SIGSEGV on detach. Integration: full lifecycle (init -> IO -> stop -> shutdown -> detach).
- **Refs**: FR-026

---

## Phase 9: Client Lifecycle

### Task 9.1: Implement Client Disconnect Detection
- **Status**: Done
- **File**: `src/actor.rs` (within `poll_clients`)
- **Description**: On `ChannelError::Closed` from ingress_rx.try_recv(), remove client via `swap_remove`. Log disconnect. Do not affect other clients.
- **Acceptance**: Integration test: drop client channels, actor continues serving other clients. No panic or deadlock.
- **Refs**: FR-019

### Task 9.2: Implement Fair Client Polling
- **Status**: Done
- **File**: `src/actor.rs` (within `poll_clients`)
- **Description**: Rotating `poll_start_idx` (wrapping_add each poll cycle). Start iteration at `start % num_clients`. Drain up to 64 commands per client per poll (high cap to avoid limiting effective QD).
- **Acceptance**: Multi-client test: no single client starves others under concurrent load.
- **Refs**: FR-020

---

## Phase 10: Testing + Benchmarks

### Task 10.1: Unit Tests
- **Status**: Done
- **Files**: All `#[cfg(test)] mod tests` blocks in each source file
- **Description**: Component metadata, version, interfaces, receptacles, error paths, defaults, QueuePair selection, TscClock calibration, namespace validation, NvmeVersion, telemetry stats, PendingOp fields.
- **Acceptance**: `cargo test --all` passes (no SPDK required for unit tests).
- **Refs**: SC-007, SC-008

### Task 10.2: Integration Tests
- **Status**: Done
- **File**: `tests/integration.rs`
- **Description**: Full wiring with SPDKEnvComponent + LoggerComponent. `OnceLock<SpdkHardwareContext>` for singleton init. Auto-skip when hardware unavailable. Tests: sync read/write, async read/write with timeout, multi-client concurrency, namespace CRUD, controller reset, abort, batch, telemetry accuracy.
- **Acceptance**: Tests pass on hardware. Tests skip gracefully without hardware.
- **Refs**: SC-001 through SC-008

### Task 10.3: Criterion Benchmarks
- **Status**: Done
- **Files**: `benches/latency.rs`, `benches/throughput.rs`
- **Description**: Latency: sync IO at QD 1/4/16/64, command construction overhead. Throughput: async IO at varying batch sizes. Hardware-gated (skip group if unavailable). Requires `--features spdk`.
- **Acceptance**: `cargo bench --bench latency --features spdk` runs. p50 sync < 100us for 4KB at QD=1 on target hardware.
- **Refs**: SC-001, NFR-009

---

## Phase 11: Documentation + Lint

### Task 11.1: Doc Comments
- **Status**: Done
- **Files**: All public types and methods
- **Description**: Comprehensive doc comments with runnable examples on all public API surface (`NvmeVersion`, `NvmeNamespaceInfo`, `QueuePair`, `QueuePairPool`). Module-level docs on `lib.rs`, `controller.rs`, `qpair.rs`, `tsc.rs`, `telemetry.rs`, `namespace.rs`, `command.rs`, `actor.rs`.
- **Acceptance**: `cargo doc --no-deps` produces zero warnings.
- **Refs**: SC-007

### Task 11.2: Lint and Format Compliance
- **Status**: Done
- **Description**: `cargo fmt --check` passes. `cargo clippy -- -D warnings` passes. All unsafe blocks have `// SAFETY:` comments.
- **Acceptance**: CI gate passes.
- **Refs**: SC-007, NFR-014
