# Tasks: Channel Backend Benchmarks

**Input**: Design documents from `/specs/004-channel-benchmarks/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Included — the spec requires unit tests (FR-014), doc tests (FR-015), and correctness tests (SC-001). Constitution mandates TDD.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Add third-party dependencies and prepare module structure

- [X] T001 Add crossbeam-channel, kanal, rtrb, and tokio (sync feature) dependencies to crates/component-core/Cargo.toml
- [X] T002 Add new channel backend submodules to crates/component-core/src/channel/mod.rs (declare `pub mod crossbeam_bounded; pub mod crossbeam_unbounded; pub mod kanal_bounded; pub mod rtrb_spsc; pub mod tokio_mpsc;`)
- [X] T003 Add re-exports for new channel backend types in crates/component-core/src/lib.rs
- [X] T004 Add new benchmark entries (channel_spsc_benchmark, channel_mpsc_benchmark, channel_latency_benchmark) to crates/component-framework/Cargo.toml

**Checkpoint**: Project compiles with new dependencies; empty modules exist for all backends

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: No additional foundational work needed — existing ISender/IReceiver/IUnknown infrastructure is already in place. Proceed to user stories.

---

## Phase 3: User Story 2 — Use Third-Party Channel Backends as Components (Priority: P2)

**Goal**: Implement crossbeam bounded, crossbeam unbounded, kanal, rtrb, and tokio channel backends as components providing ISender/IReceiver via IUnknown

**Independent Test**: Create each backend, query ISender/IReceiver, send/receive messages, verify binding enforcement and introspection

**Note**: US2 is implemented before US1 because the benchmarks (US1) depend on having the backends available.

### Implementation for User Story 2

- [X] T005 [P] [US2] Implement CrossbeamBoundedChannel<T> with ISender/IReceiver wrappers, IUnknown impl, unit tests, and doc tests in crates/component-core/src/channel/crossbeam_bounded.rs
- [X] T006 [P] [US2] Implement CrossbeamUnboundedChannel<T> with ISender/IReceiver wrappers, IUnknown impl, unit tests, and doc tests in crates/component-core/src/channel/crossbeam_unbounded.rs
- [X] T007 [P] [US2] Implement KanalChannel<T> with ISender/IReceiver wrappers, IUnknown impl, unit tests, and doc tests in crates/component-core/src/channel/kanal_bounded.rs
- [X] T008 [P] [US2] Implement RtrbChannel<T> (SPSC only) with Mutex-wrapped Producer/Consumer ISender/IReceiver wrappers, IUnknown impl (SPSC binding enforcement), unit tests, and doc tests in crates/component-core/src/channel/rtrb_spsc.rs
- [X] T009 [P] [US2] Implement TokioMpscChannel<T> with blocking_send/blocking_recv ISender/IReceiver wrappers, IUnknown impl, unit tests, and doc tests in crates/component-core/src/channel/tokio_mpsc.rs
- [X] T010 [US2] Add integration tests for all backends (send/recv correctness, binding enforcement, IUnknown introspection, 100K message zero-loss) in crates/component-framework/tests/channel_backends.rs
- [X] T011 [US2] Run CI gate: `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps`

**Checkpoint**: All 5 backends pass unit tests, doc tests, integration tests, and CI gate. Each backend is a drop-in replacement for built-in channels via ISender/IReceiver.

---

## Phase 4: User Story 1 — Compare Channel Performance (Priority: P1)

**Goal**: Build Criterion benchmark suite comparing throughput and latency of all channel backends under standardized conditions

**Independent Test**: Run `cargo bench` and verify results are produced for every backend in consistent format showing throughput and latency

### Implementation for User Story 1

- [X] T012 [P] [US1] Implement SPSC throughput benchmarks comparing built-in, crossbeam-bounded, crossbeam-unbounded, kanal, and rtrb with small (u64) and large (Vec<u8> 1024B) messages at queue capacities 64, 1024, 16384 in crates/component-framework/benches/channel_spsc_benchmark.rs
- [X] T013 [P] [US1] Implement MPSC throughput benchmarks comparing built-in, crossbeam-bounded, crossbeam-unbounded, kanal, and tokio with 2, 4, 8 producers, small and large messages, at queue capacities 64, 1024, 16384 in crates/component-framework/benches/channel_mpsc_benchmark.rs
- [X] T014 [P] [US1] Implement latency benchmarks measuring per-message send-to-receive time for all backends in SPSC and MPSC modes in crates/component-framework/benches/channel_latency_benchmark.rs
- [X] T015 [US1] Verify benchmark suite runs successfully with `cargo bench` and produces results for all backends
- [X] T016 [US1] Run CI gate: `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps`

**Checkpoint**: Complete benchmark suite runs in under 5 minutes; results cover all backends, topologies, message sizes

---

## Phase 5: User Story 3 — Benchmark Different Topologies (Priority: P3)

**Goal**: Ensure benchmarks cover varying producer counts and queue depths to reveal topology-specific performance characteristics

**Independent Test**: Run topology-specific benchmark groups and verify results include multiple producer counts and queue capacities

**Note**: This is largely satisfied by the benchmark implementations in Phase 4 (T012, T013). This phase validates completeness and adds any missing configurations.

### Implementation for User Story 3

- [X] T017 [US3] Verify SPSC benchmarks include at least 2 queue capacities per backend in crates/component-framework/benches/channel_spsc_benchmark.rs
- [X] T018 [US3] Verify MPSC benchmarks include 2, 4, and 8 producer counts per backend in crates/component-framework/benches/channel_mpsc_benchmark.rs
- [X] T019 [US3] Verify queue depth variation (64, 1024, 16384) is included in both SPSC and MPSC benchmarks
- [X] T020 [US3] Run full benchmark suite and validate output covers all required topology/capacity combinations

**Checkpoint**: Benchmark suite produces results for all topology × capacity × message size × producer count combinations

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Final validation and cleanup

- [X] T021 Update existing channel_throughput benchmark to note that comprehensive benchmarks are in the new benchmark files in crates/component-framework/benches/channel_throughput.rs
- [X] T022 Run full CI gate: `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps`
- [X] T023 Run quickstart.md code examples manually to verify they compile and work
- [X] T024 Verify all public types and functions have doc tests per constitution requirement

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **US2 (Phase 3)**: Depends on Setup — implements all backends
- **US1 (Phase 4)**: Depends on US2 — benchmarks require backends to exist
- **US3 (Phase 5)**: Depends on US1 — validates benchmark coverage
- **Polish (Phase 6)**: Depends on all user stories complete

### User Story Dependencies

- **User Story 2 (P2)**: Depends on Setup only — implements backends
- **User Story 1 (P1)**: Depends on US2 — cannot benchmark backends that don't exist yet
- **User Story 3 (P3)**: Depends on US1 — validates benchmark topology coverage

### Within Each User Story

- Backend implementations (T005–T009) can run in parallel [P]
- Benchmark implementations (T012–T014) can run in parallel [P]
- Integration tests and CI gates run after implementation

### Parallel Opportunities

**Phase 3 (US2)**: All 5 backend implementations (T005–T009) can run in parallel — each is an independent module
**Phase 4 (US1)**: All 3 benchmark files (T012–T014) can run in parallel — each is an independent bench file

---

## Parallel Example: User Story 2

```bash
# Launch all backend implementations together (each is a separate file):
Task T005: "Implement CrossbeamBoundedChannel in crossbeam_bounded.rs"
Task T006: "Implement CrossbeamUnboundedChannel in crossbeam_unbounded.rs"
Task T007: "Implement KanalChannel in kanal_bounded.rs"
Task T008: "Implement RtrbChannel in rtrb_spsc.rs"
Task T009: "Implement TokioMpscChannel in tokio_mpsc.rs"
```

## Parallel Example: User Story 1

```bash
# Launch all benchmark files together:
Task T012: "SPSC throughput benchmarks in channel_spsc_benchmark.rs"
Task T013: "MPSC throughput benchmarks in channel_mpsc_benchmark.rs"
Task T014: "Latency benchmarks in channel_latency_benchmark.rs"
```

---

## Implementation Strategy

### MVP First (User Story 2 → User Story 1)

1. Complete Phase 1: Setup (add deps, create modules)
2. Complete Phase 3: US2 (implement all 5 backends)
3. **STOP and VALIDATE**: All backends pass `cargo test --all`
4. Complete Phase 4: US1 (implement benchmarks)
5. **STOP and VALIDATE**: `cargo bench` produces results for all backends
6. Complete Phase 5: US3 (validate coverage)
7. Complete Phase 6: Polish

### Incremental Delivery

1. Setup → Foundation ready
2. Implement 1 backend (e.g., CrossbeamBounded) → validate pattern works
3. Implement remaining 4 backends in parallel → all pass tests
4. Implement benchmarks → all backends benchmarked
5. Polish → CI gate passes

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- US2 (backends) must precede US1 (benchmarks) despite P2 > P1 priority — benchmarks need backends
- Each backend follows the same OnceLock + CAS pattern as existing SpscChannel/MpscChannel
- rtrb requires Mutex wrappers (Producer/Consumer are !Sync)
- tokio uses blocking_send/blocking_recv (no async runtime)
- Crossbeam unbounded: try_send always succeeds (never Full)
- Commit after each task or logical group
