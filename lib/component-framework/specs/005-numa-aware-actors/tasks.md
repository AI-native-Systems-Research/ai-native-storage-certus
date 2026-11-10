# Tasks: NUMA-Aware Actor Thread Pinning and Memory Allocation

**Input**: Design documents from `/specs/005-numa-aware-actors/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Included — the project constitution mandates TDD with comprehensive testing (unit, integration, doc tests).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3, US4)
- Include exact file paths in descriptions

## Phase 1: Setup

**Purpose**: Add `libc` dependency and create `numa` module skeleton

- [X] T001 Add `libc = "0.2"` to `[dependencies]` in `crates/component-core/Cargo.toml`
- [X] T002 Create `crates/component-core/src/numa/mod.rs` with module declarations, `NumaError` enum, and re-exports
- [X] T003 [P] Create empty `crates/component-core/src/numa/cpuset.rs` with `CpuSet` struct stub
- [X] T004 [P] Create empty `crates/component-core/src/numa/topology.rs` with `NumaTopology` and `NumaNode` struct stubs
- [X] T005 [P] Create empty `crates/component-core/src/numa/allocator.rs` with `NumaAllocator` struct stub
- [X] T006 Add `pub mod numa;` and public exports to `crates/component-core/src/lib.rs`

---

## Phase 2: Foundational — CpuSet and NumaError

**Purpose**: Core types that ALL user stories depend on — MUST complete before any story phase

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [X] T007 Implement `NumaError` enum with variants `CpuOutOfRange`, `CpuOffline`, `EmptyCpuSet`, `InvalidNode`, `TopologyUnavailable`, `AffinityFailed`, `AllocationFailed` and `Display`/`Error` impls with doc tests in `crates/component-core/src/numa/mod.rs`
- [X] T008 Implement `CpuSet` struct wrapping `libc::cpu_set_t` with `new()`, `from_cpu()`, `from_cpus()`, `add()`, `remove()`, `contains()`, `count()`, `is_empty()`, `iter()`, `as_raw()` methods and doc tests for each in `crates/component-core/src/numa/cpuset.rs`
- [X] T009 Add unit tests for `CpuSet`: empty set detection, add/remove/contains, from_cpu with valid and invalid IDs, from_cpus, iterator correctness, count accuracy in `crates/component-core/src/numa/cpuset.rs`
- [X] T010 Add `ActorError::AffinityFailed(String)` variant to `crates/component-core/src/actor.rs` and update Display impl

**Checkpoint**: `CpuSet` and `NumaError` complete — `cargo test --all` passes, `cargo doc --no-deps` clean

---

## Phase 3: User Story 1 — Pin Actor Thread to Specific CPUs (Priority: P1) 🎯 MVP

**Goal**: Actors can be configured with a CPU affinity set and their dedicated thread is pinned to those CPUs on activation

**Independent Test**: Create an actor with CPU affinity, activate it, send a message, verify the actor's thread runs on the specified CPU(s)

### Tests for User Story 1

- [X] T011 [US1] Write unit tests for `set_cpu_affinity()`: valid set while idle, reject while running (`AlreadyActive`), verify stored affinity in `crates/component-core/src/actor.rs`
- [X] T012 [US1] Write integration test: actor pinned to CPU 0, send message, verify `sched_getaffinity` reports CPU 0 inside handler in `crates/component-core/tests/numa_integration.rs`
- [X] T013 [US1] Write integration test: actor with no affinity (default) behaves identically to existing actors — backward compatibility in `crates/component-core/tests/numa_integration.rs`
- [X] T014 [US1] Write unit test: actor with invalid CPU ID (e.g., 9999) returns error on activate in `crates/component-core/tests/numa_integration.rs`
- [X] T015 [US1] Write unit test: actor with empty CpuSet returns error on activate in `crates/component-core/tests/numa_integration.rs`

### Implementation for User Story 1

- [X] T016 [US1] Implement safe wrapper `fn set_thread_affinity(cpuset: &CpuSet) -> Result<(), NumaError>` calling `libc::sched_setaffinity(0, ...)` in `crates/component-core/src/numa/cpuset.rs`
- [X] T017 [US1] Implement safe wrapper `fn get_thread_affinity() -> Result<CpuSet, NumaError>` calling `libc::sched_getaffinity(0, ...)` in `crates/component-core/src/numa/cpuset.rs`
- [X] T018 [US1] Implement `fn validate_cpus(cpuset: &CpuSet) -> Result<(), NumaError>` that checks all CPUs in set are online (read `/sys/devices/system/cpu/online`) in `crates/component-core/src/numa/cpuset.rs`
- [X] T019 [US1] Add `cpu_affinity: Mutex<Option<CpuSet>>` field to `Actor` struct, add `with_cpu_affinity(self, affinity: CpuSet) -> Self` builder method, add `set_cpu_affinity(&self, affinity: CpuSet) -> Result<(), ActorError>` and `cpu_affinity(&self) -> Option<CpuSet>` accessor in `crates/component-core/src/actor.rs`
- [X] T020 [US1] Modify `Actor::activate()` to pass `cpu_affinity` clone into spawned thread, call `set_thread_affinity()` before message loop, propagate errors back via channel in `crates/component-core/src/actor.rs`
- [X] T021 [US1] Add doc tests for `Actor::with_cpu_affinity()`, `Actor::set_cpu_affinity()`, and `Actor::cpu_affinity()` in `crates/component-core/src/actor.rs`
- [X] T022 [US1] Run `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps` to verify US1 complete

**Checkpoint**: Actor thread pinning works — an actor can be pinned to specific CPUs and verified. All existing tests still pass.

---

## Phase 4: User Story 2 — Query NUMA Topology at Runtime (Priority: P1)

**Goal**: Users can discover NUMA nodes, their CPUs, and inter-node distances at runtime

**Independent Test**: Query topology, verify at least 1 node with at least 1 CPU, all online CPUs accounted for

### Tests for User Story 2

- [X] T023 [P] [US2] Write unit test for range-list parser: parse "0-15,32-47" → Vec<usize>, edge cases "0", "0-0", empty string in `crates/component-core/src/numa/topology.rs`
- [X] T024 [P] [US2] Write unit test: `NumaTopology::discover()` returns at least 1 node, each node has non-empty cpus, all online CPUs appear exactly once in `crates/component-core/tests/numa_integration.rs`
- [X] T025 [P] [US2] Write unit test: `node_for_cpu()` returns correct node for known CPUs, `None` for invalid CPU in `crates/component-core/tests/numa_integration.rs`

### Implementation for User Story 2

- [X] T026 [US2] Implement `fn parse_range_list(s: &str) -> Result<Vec<usize>, NumaError>` for parsing sysfs cpulist format in `crates/component-core/src/numa/topology.rs`
- [X] T027 [US2] Implement `NumaNode` struct with `id()`, `cpus()`, `distances()`, `distance_to()` methods and doc tests in `crates/component-core/src/numa/topology.rs`
- [X] T028 [US2] Implement `NumaTopology::discover()` reading `/sys/devices/system/node/online`, `/sys/devices/system/node/nodeN/cpulist`, and `/sys/devices/system/node/nodeN/distance` with fallback to single-node if sysfs unavailable (FR-009) in `crates/component-core/src/numa/topology.rs`
- [X] T029 [US2] Implement `NumaTopology` methods: `node_count()`, `node()`, `nodes()`, `node_for_cpu()`, `online_cpus()` with doc tests in `crates/component-core/src/numa/topology.rs`
- [X] T030 [US2] Run `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps` to verify US2 complete

**Checkpoint**: Topology discovery works — users can query nodes, CPUs, and distances. Works on both multi-NUMA and single-NUMA systems.

---

## Phase 5: User Story 3 — NUMA-Aware Performance Benchmarks (Priority: P2)

**Goal**: Benchmarks measure actor-to-actor latency and throughput for same-node and cross-node configurations, including NUMA-local vs default allocation comparison

**Independent Test**: Run benchmark suite, verify it produces labeled results for both configurations, cross-node latency > same-node latency

**Depends on**: US1 (thread pinning), US2 (topology discovery)

### Implementation for User Story 3 — NUMA Allocator

- [X] T031 [US3] Implement `NumaAllocator::new(node_id)`, `alloc(layout)` using `mmap` + `syscall(SYS_mbind, MPOL_BIND)`, `dealloc(ptr, layout)` using `munmap`, with fallback on mbind failure (FR-019) and doc tests in `crates/component-core/src/numa/allocator.rs`
- [X] T032 [US3] Write unit tests for `NumaAllocator`: allocate and deallocate successfully, allocation on valid node, fallback behavior in `crates/component-core/src/numa/allocator.rs`
- [X] T033 [US3] Implement `MpscChannel::new_numa(capacity, node)` and `SpscChannel::new_numa(capacity, node)` using first-touch NUMA policy in `crates/component-core/src/channel/mpsc.rs` and `crates/component-core/src/channel/spsc.rs`
- [X] T034 [US3] NUMA-local channel allocation via first-touch policy (pin thread before constructing channel); `with_numa_node` on Actor deferred — users pin via `with_cpu_affinity` + first-touch
- [X] T035 [US3] Write unit tests for `new_numa()` constructors: channel works correctly with NUMA-allocated buffer, send/receive 1000 messages in `crates/component-core/tests/numa_integration.rs`

### Implementation for User Story 3 — Benchmarks

- [X] T036 [US3] Create `crates/component-framework/benches/numa_latency_benchmark.rs` with benchmark groups: `numa_latency/spsc/{same_node, cross_node, same_node_numa_alloc, cross_node_numa_alloc}` using `iter_custom` with pre-pinned threads, topology detection, and graceful skip on single-NUMA systems
- [X] T037 [US3] Create `crates/component-framework/benches/numa_throughput_benchmark.rs` with benchmark groups: `numa_throughput/spsc/{same_node, cross_node, same_node_numa_alloc, cross_node_numa_alloc}` measuring messages-per-second with same methodology as existing channel benchmarks
- [X] T038 [US3] Register both benchmark files in `crates/component-framework/Cargo.toml` under `[[bench]]` sections with `harness = false`
- [X] T039 [US3] Run `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps && cargo bench --bench numa_latency_benchmark -- --list && cargo bench --bench numa_throughput_benchmark -- --list` to verify US3 complete

**Checkpoint**: NUMA allocator works, benchmarks compile and discover all cases. On a 2+ NUMA system, cross-node latency is measurably higher than same-node.

---

## Phase 6: User Story 4 — NUMA Pinning Example (Priority: P3)

**Goal**: Self-contained example demonstrating topology discovery, actor pinning, message exchange, and latency reporting

**Independent Test**: Example compiles, runs without errors, prints topology and latency measurements

**Depends on**: US1, US2, US3

- [X] T040 [US4] Create `examples/numa_pinning.rs` that: (1) discovers NUMA topology and prints it, (2) pins two threads to same-node CPUs and measures round-trip latency, (3) pins two threads to cross-node CPUs and measures round-trip latency, (4) prints comparison, (5) on single-NUMA system prints warning and skips cross-node test
- [X] T041 [US4] Verify example compiles and runs: `cargo run --example numa_pinning`

**Checkpoint**: Example is a complete, runnable demonstration of all NUMA features.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Final validation across all stories

- [X] T042 Run full CI gate: `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps`
- [X] T043 Verify all existing tests from features 001-004 pass unchanged (backward compatibility SC-004)
- [X] T044 Verify all public APIs in `numa` module have doc tests with runnable examples (constitution IV)
- [X] T045 Review all `unsafe` blocks have justification comments and safety invariant tests (constitution I)
- [X] T046 Run quickstart.md code snippets mentally against implemented API to verify accuracy

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational — thread pinning
- **US2 (Phase 4)**: Depends on Foundational — topology discovery (can run in parallel with US1)
- **US3 (Phase 5)**: Depends on US1 + US2 — benchmarks need pinning and topology
- **US4 (Phase 6)**: Depends on US1 + US2 + US3 — example uses all features
- **Polish (Phase 7)**: Depends on all user stories complete

### User Story Dependencies

```text
Phase 1: Setup
    │
Phase 2: Foundational (CpuSet, NumaError)
    │
    ├──► Phase 3: US1 (Thread Pinning) ──┐
    │                                     │
    ├──► Phase 4: US2 (Topology) ─────────┤
    │                                     │
    │                                     ▼
    │                              Phase 5: US3 (Benchmarks + Allocator)
    │                                     │
    │                                     ▼
    │                              Phase 6: US4 (Example)
    │                                     │
    └─────────────────────────────────────▼
                                   Phase 7: Polish
```

### Within Each User Story

- Tests written and verified to fail before implementation
- Types/models before services/logic
- Safe wrappers before integration
- Doc tests alongside implementation
- CI gate at end of each story phase

### Parallel Opportunities

- **Phase 1**: T003, T004, T005 can run in parallel (separate files)
- **Phase 2**: T007, T008 are sequential (T008 depends on T007 for NumaError); T009 parallel with T010
- **Phase 3 + Phase 4**: US1 and US2 can run in parallel after Foundational completes
- **Phase 4**: T023, T024, T025 can run in parallel (separate test files)

---

## Parallel Example: User Story 1 + User Story 2

```bash
# After Phase 2 completes, launch US1 and US2 in parallel:

# US1 stream:
Task: T011-T015 (tests), then T016-T022 (implementation)

# US2 stream (parallel):
Task: T023-T025 (tests), then T026-T030 (implementation)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001-T006)
2. Complete Phase 2: Foundational (T007-T010)
3. Complete Phase 3: User Story 1 — Thread Pinning (T011-T022)
4. **STOP and VALIDATE**: Actor pinning works, all existing tests pass
5. Deliverable: Actors can be pinned to specific CPUs

### Incremental Delivery

1. Setup + Foundational → Core types ready
2. Add US1 (Thread Pinning) → Test independently → MVP!
3. Add US2 (Topology Discovery) → Test independently → Users can discover NUMA layout
4. Add US3 (Benchmarks + Allocator) → Test independently → Quantified NUMA effects
5. Add US4 (Example) → Complete → Full documentation and learning material

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- US1 and US2 are both P1 and can be developed in parallel after Foundational
- US3 depends on both US1 and US2 — cannot start until both complete
- All unsafe code must have `// SAFETY:` comments per constitution
- `cargo test --all` must pass after every phase checkpoint
- Commit after each task or logical group
