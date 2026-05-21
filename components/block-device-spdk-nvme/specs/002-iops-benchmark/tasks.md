# Tasks: IOPS Benchmark Example Application

**Input**: Design documents from `/specs/002-iops-benchmark/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/cli-contract.md

**Tests**: Included per constitution mandate (Principle II: "Every public function, method, and type MUST have unit tests").

**Organization**: Tasks grouped by user story for independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story (US1, US2, US3, US4)
- Exact file paths relative to `apps/iops-benchmark/`

---

## Phase 1: Setup

**Purpose**: Create the binary crate and register it in the workspace.

- [x] T001 Create `apps/iops-benchmark/Cargo.toml` with dependencies: block-device-spdk-nvme, component-framework, interfaces, spdk-env, example-logger, clap (features=["derive"]), rand
- [x] T002 Create `apps/iops-benchmark/src/main.rs` with skeleton `fn main()` that prints "iops-benchmark" and exits
- [x] T003 Register `apps/iops-benchmark` in workspace `Cargo.toml` under `[workspace] members`
- [x] T004 Verify `cargo build -p iops-benchmark` compiles successfully

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and modules that ALL user stories depend on.

**CRITICAL**: No user story work can begin until this phase is complete.

- [x] T005 [P] Create `apps/iops-benchmark/src/config.rs` with `OpType` enum (Read, Write, ReadWrite), `Pattern` enum (Random, Sequential), and `BenchConfig` struct with all fields from data-model.md. Derive `clap::Parser` for CLI argument parsing with defaults (op=read, block_size=4096, queue_depth=32, threads=1, duration=10, ns_id=1, pattern=random, quiet=false). Add `--pci-addr` as `Option<String>`.
- [x] T006 [P] Create `apps/iops-benchmark/src/stats.rs` with `ThreadResult` struct (read_ops, write_ops, errors, latencies_ns: Vec<u64>) and `FinalReport` struct with all fields from data-model.md. Implement `FinalReport::from_results(results: &[ThreadResult], duration_secs: f64, block_size: usize)` that computes IOPS, throughput, and latency percentiles (min, mean, p50, p99, max) by sorting merged latency samples.
- [x] T007 [P] Create `apps/iops-benchmark/src/report.rs` with three functions: `print_config(config, ns_info)` (config summary to stdout), `print_progress(elapsed_secs, instant_iops)` (one-line to stderr), `print_final(report, op_type)` (formatted results to stdout, with separate read/write lines for ReadWrite mode). Follow output format from contracts/cli-contract.md.
- [x] T008 [P] Create `apps/iops-benchmark/src/lba.rs` with trait `LbaGenerator` and two implementations: `RandomLba` (uniform random via `rand::thread_rng().gen_range(0..max_lba)`) and `SequentialLba` (contiguous with wrap-around, constructed with thread_index, num_threads, total_sectors, blocks_per_io).
- [x] T009 Unit tests for `stats.rs`: test `FinalReport::from_results` with known inputs — verify IOPS calculation, throughput, percentile accuracy (exact values on small sample set), and empty-results edge case
- [x] T010 [P] Unit tests for `lba.rs`: test `RandomLba` produces values in valid range, test `SequentialLba` produces contiguous values, wraps correctly, and threads get non-overlapping regions
- [x] T011 Wire modules in `apps/iops-benchmark/src/main.rs`: add `mod config; mod stats; mod report; mod lba; mod worker;` declarations
- [x] T012 Verify `cargo build -p iops-benchmark` and `cargo test -p iops-benchmark` pass

**Checkpoint**: Foundation ready — all shared types compiled and tested.

---

## Phase 3: User Story 1 — Run a Basic IOPS Benchmark (Priority: P1) MVP

**Goal**: Launch with defaults, run async IO for the configured duration, print IOPS/throughput/latency summary.

**Independent Test**: Run `iops-benchmark` with no arguments on a host with SPDK hardware. After 10 seconds, IOPS and latency results print to stdout.

### Implementation for User Story 1

- [x] T013 [US1] Create `apps/iops-benchmark/src/worker.rs` with `Worker` struct holding: `Arc<BenchConfig>`, `ClientChannels`, `NamespaceInfo`, `Vec<Arc<Mutex<DmaBuffer>>>` (read buffers) and `Vec<Arc<DmaBuffer>>` (write buffers) pre-allocated to queue_depth, `HashMap<OpHandle, Instant>` for in-flight tracking, `Arc<AtomicU64>` for live op counter, `Arc<AtomicBool>` stop flag.
- [x] T014 [US1] Implement `Worker::new()` in `apps/iops-benchmark/src/worker.rs`: call `ibd.connect_client()`, send `NsProbe` + `flush_io()` + recv to get namespace info, allocate DMA buffers (block_size each, one per queue_depth slot), initialize LBA generator (RandomLba for now).
- [x] T015 [US1] Implement `Worker::run(&mut self) -> ThreadResult` in `apps/iops-benchmark/src/worker.rs`: tight loop that (1) submits ReadAsync/WriteAsync up to queue_depth outstanding, using `flush_io()` after each batch of sends, (2) drains completions via `try_recv` loop recording latency from in-flight HashMap, (3) re-submits to keep pipeline full, (4) checks stop flag each iteration, (5) returns ThreadResult on exit.
- [x] T016 [US1] Implement component wiring in `apps/iops-benchmark/src/main.rs`: parse CLI args, create LoggerComponent + SPDKEnvComponent + BlockDeviceSpdkNvmeComponent, bind receptacles, init SPDK env, select first device (--pci-addr support deferred to US2), set PCI address, initialize block device, query IBlockDevice.
- [x] T017 [US1] Implement thread orchestration in `apps/iops-benchmark/src/main.rs`: create `Arc<AtomicBool>` stop flag, spawn worker thread (single thread for MVP), spawn timer thread that sleeps `duration_secs` then sets stop flag, join worker, compute FinalReport, call `print_config` and `print_final`.
- [x] T018 [US1] Verify `cargo build -p iops-benchmark --release` compiles. On hardware: run `sudo ./target/release/iops-benchmark` and confirm IOPS output after 10 seconds.

**Checkpoint**: User Story 1 complete — default 4KB random read benchmark works end-to-end.

---

## Phase 4: User Story 2 — Configure Workload Parameters (Priority: P1)

**Goal**: All CLI flags (`--op`, `--block-size`, `--queue-depth`, `--threads`, `--pattern`, `--pci-addr`, `--ns-id`, `--duration`) work correctly and are reflected in output.

**Independent Test**: Run with `--op write --block-size 65536 --threads 4 --queue-depth 64 --duration 5 --pattern sequential` and confirm config summary shows all parameters and 4 threads run concurrently.

### Implementation for User Story 2

- [x] T019 [US2] Add `--op write` and `--op rw` support in `apps/iops-benchmark/src/worker.rs`: in the submission loop, select ReadAsync or WriteAsync based on OpType. For ReadWrite, use `rand::thread_rng().gen_bool(0.5)` to choose per-op. Track read_ops/write_ops separately in ThreadResult.
- [x] T020 [US2] Add `--pattern sequential` support: update `Worker::new()` in `apps/iops-benchmark/src/worker.rs` to construct `SequentialLba` when `config.pattern == Pattern::Sequential`, passing thread_index and num_threads for non-overlapping regions.
- [x] T021 [US2] Add multi-thread support in `apps/iops-benchmark/src/main.rs`: spawn `config.threads` worker threads (each calls `ibd.connect_client()` for its own channels), join all, aggregate all ThreadResults into FinalReport.
- [x] T022 [US2] Add `--pci-addr` device selection in `apps/iops-benchmark/src/main.rs`: parse BDF string into domain/bus/dev/func, match against `ienv.devices()` list. If not found, print error and exit with code 2. If not specified, use first device.
- [x] T023 [US2] Add separate read/write IOPS reporting in `apps/iops-benchmark/src/report.rs`: in `print_final`, when OpType is ReadWrite, print "Read ops: N (X IOPS)" and "Write ops: N (X IOPS)" lines before total (per contracts/cli-contract.md format).
- [x] T024 [US2] Verify: run with `--op rw --threads 2 --pattern sequential --block-size 8192 --duration 5` and confirm config summary, separate read/write IOPS in output, and correct block size.

**Checkpoint**: User Story 2 complete — all workload parameters are configurable.

---

## Phase 5: User Story 3 — View Live Progress (Priority: P2)

**Goal**: Per-second progress lines on stderr during the benchmark; `--quiet` suppresses them.

**Independent Test**: Run a 10-second benchmark and observe per-second IOPS lines on stderr. Re-run with `--quiet` and confirm no progress output.

### Implementation for User Story 3

- [x] T025 [US3] Add `Arc<AtomicU64>` shared op counter to `Worker` in `apps/iops-benchmark/src/worker.rs`: increment via `fetch_add(1, Relaxed)` on each successful completion. Expose via `Worker::op_counter() -> Arc<AtomicU64>`.
- [x] T026 [US3] Implement progress reporter in `apps/iops-benchmark/src/main.rs`: if `!config.quiet`, main thread loops every 1 second reading all workers' atomic counters, computing delta from previous read as instantaneous IOPS, calling `report::print_progress(elapsed, iops)`. Loop exits when stop flag is set.
- [x] T027 [US3] Verify: run `iops-benchmark --duration 5` and confirm 5 progress lines on stderr. Run with `--quiet` and confirm no progress lines.

**Checkpoint**: User Story 3 complete — live progress feedback works.

---

## Phase 6: User Story 4 — Validate Configuration (Priority: P3)

**Goal**: Invalid parameters are caught at startup with clear errors. Queue depth is clamped with a warning.

**Independent Test**: Run with `--block-size 1000` (not a multiple of sector size) and confirm error message + exit code 1.

### Implementation for User Story 4

- [x] T028 [US4] Implement `BenchConfig::validate(&self, sector_size: u32, max_qd: u32, ns_list: &[NamespaceInfo]) -> Result<(), String>` in `apps/iops-benchmark/src/config.rs`: check block_size > 0 and block_size % sector_size == 0, threads >= 1, duration >= 1, queue_depth >= 1, ns_id exists in ns_list. Return descriptive error string on failure.
- [x] T029 [US4] Implement queue depth clamping in `apps/iops-benchmark/src/config.rs`: `BenchConfig::clamp_queue_depth(&mut self, max_qd: u32)` that prints warning to stderr and clamps if queue_depth > max_qd.
- [x] T030 [US4] Wire validation into `apps/iops-benchmark/src/main.rs`: after device init and NsProbe, call `config.validate()`. On error, print to stderr and exit with code 1. Call `config.clamp_queue_depth()` before spawning workers.
- [x] T031 [US4] Unit tests for validation in `apps/iops-benchmark/src/config.rs`: test valid config passes, test block_size not multiple of sector_size fails, test zero threads/duration fails, test invalid ns_id fails, test queue depth clamping.
- [x] T032 [US4] Exit code handling in `apps/iops-benchmark/src/main.rs`: use `std::process::exit(1)` for validation errors, `exit(2)` for fatal errors (device not found, SPDK init failed, DMA alloc failed), `exit(0)` on success (per contracts/cli-contract.md).

**Checkpoint**: User Story 4 complete — robust parameter validation.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Quality gates, documentation, and final verification.

- [x] T033 [P] Add doc comments to all public types and functions in `apps/iops-benchmark/src/config.rs`, `stats.rs`, `report.rs`, `lba.rs`, `worker.rs`
- [x] T034 [P] Add crate-level doc comment in `apps/iops-benchmark/src/main.rs` describing the benchmark application purpose and usage
- [x] T035 Run `cargo fmt --check` on `apps/iops-benchmark/` and fix any formatting issues
- [x] T036 Run `cargo clippy -p iops-benchmark -- -D warnings` and fix all warnings
- [x] T037 Run `cargo test -p iops-benchmark` and verify all unit tests pass
- [x] T038 Run `cargo doc -p iops-benchmark --no-deps` and verify zero warnings
- [x] T039 Validate against quickstart.md: build with `cargo build -p iops-benchmark --release`, run default benchmark, run custom parameter benchmark

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 completion — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Phase 2 — MVP, do this first
- **US2 (Phase 4)**: Depends on Phase 3 (extends worker and main from US1)
- **US3 (Phase 5)**: Depends on Phase 3 (adds progress to existing worker/main)
- **US4 (Phase 6)**: Depends on Phase 2 only (validation is independent of IO logic, but practically best after US1 for integration testing)
- **Polish (Phase 7)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (P1)**: Blocks US2 and US3 (they extend US1's worker and main)
- **US2 (P1)**: Extends US1, can start after US1 checkpoint
- **US3 (P2)**: Extends US1, can start after US1 checkpoint. Independent of US2.
- **US4 (P3)**: Can start after Phase 2, independent of US1-US3 (but integration benefits from US1)

### Within Each Phase

- Tasks marked [P] can run in parallel
- Sequential tasks must complete in order

### Parallel Opportunities

Phase 2: T005, T006, T007, T008 are all independent files — run in parallel
Phase 2: T009, T010 (tests) can run in parallel after their respective modules
After US1: US2 and US3 can proceed in parallel (US2 extends worker/main, US3 adds progress — different concerns)

---

## Parallel Example: Phase 2 (Foundational)

```
# Launch all module creation tasks together:
T005: config.rs
T006: stats.rs
T007: report.rs
T008: lba.rs

# Then launch tests together:
T009: stats tests
T010: lba tests
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001-T004)
2. Complete Phase 2: Foundational (T005-T012)
3. Complete Phase 3: User Story 1 (T013-T018)
4. **STOP and VALIDATE**: Run default benchmark on hardware, confirm IOPS output
5. This is a deployable, useful benchmark tool

### Incremental Delivery

1. Setup + Foundational -> Foundation ready
2. Add US1 -> Basic benchmark works (MVP!)
3. Add US2 -> All parameters configurable
4. Add US3 -> Live progress feedback
5. Add US4 -> Robust validation
6. Polish -> Quality gates pass

---

## Notes

- The benchmark binary requires SPDK hardware to run. Unit tests (stats, lba, config validation) run without hardware.
- `flush_io()` must be called after every batch of commands sent on `command_tx` — this wakes the actor thread.
- Channel capacity is 64 slots. Worker must interleave send/recv to avoid blocking when queue_depth > 64.
- DmaBuffer for reads needs `Arc<Mutex<DmaBuffer>>`, for writes needs `Arc<DmaBuffer>` (asymmetric API).
- Constitution requires `cargo fmt + clippy + test + doc + bench --no-run` to pass before merge.
