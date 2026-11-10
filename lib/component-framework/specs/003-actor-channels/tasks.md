# Tasks: Actor Model with Channel Components

**Input**: Design documents from `/specs/003-actor-channels/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/public-api.md, quickstart.md

**Tests**: Constitution mandates TDD — test tasks are included. Write tests FIRST, ensure they FAIL, then implement.

**Organization**: Tasks grouped by user story (P1→P2→P3) for independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1–US5)
- Include exact file paths in descriptions

## Path Conventions

- **component-core**: `crates/component-core/src/`
- **component-macros**: `crates/component-macros/src/`
- **component-framework**: `crates/component-framework/`
- **examples**: `examples/`

---

## Phase 1: Setup

**Purpose**: Module scaffolding and error type extensions

- [X] T001 Add `actor` module declaration and `channel` module hierarchy to `crates/component-core/src/lib.rs` (add `pub mod actor; pub mod channel;` and re-exports)
- [X] T002 [P] Create `crates/component-core/src/channel/mod.rs` with `ChannelError` enum, `ISender` trait, `IReceiver` trait, and sub-module declarations (`pub mod queue; pub mod spsc; pub mod mpsc;`)
- [X] T003 [P] Create `crates/component-core/src/actor.rs` with `ActorError` enum, `ActorHandler` trait, `Actor` struct skeleton, and `ActorHandle` struct skeleton
- [X] T004 [P] Create placeholder files `crates/component-core/src/channel/queue.rs`, `crates/component-core/src/channel/spsc.rs`, `crates/component-core/src/channel/mpsc.rs` (empty modules that compile)
- [X] T005 Extend `crates/component-framework/src/lib.rs` to re-export new actor and channel types
- [X] T006 Verify `cargo test --all` passes with empty new modules (no regressions)

**Checkpoint**: All new modules exist and compile. Existing tests pass unchanged.

---

## Phase 2: Foundational (Lock-Free Ring Buffer)

**Purpose**: The SPSC ring buffer is the core primitive used by all channels and actors. MUST complete before any user story.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

### Tests

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [X] T007 Write unit tests for SPSC ring buffer in `crates/component-core/src/channel/queue.rs` — test: new() with power-of-two capacity, push/pop single element, push/pop to capacity, push on full returns false, pop on empty returns None, FIFO ordering with 1000 elements, Send+Sync bounds
- [X] T008 [P] Write unit test for ring buffer closure signaling in `crates/component-core/src/channel/queue.rs` — test: after sender_alive set to false, pop returns None once empty
- [X] T009 [P] Write concurrency stress test for ring buffer in `crates/component-core/src/channel/queue.rs` — test: producer thread pushes 100K items, consumer thread pops all, verify zero loss and FIFO order

### Implementation

- [X] T010 Implement `CachePadded<T>` wrapper struct in `crates/component-core/src/channel/queue.rs` (align to 64 bytes to prevent false sharing)
- [X] T011 Implement `RingBuffer<T>` struct in `crates/component-core/src/channel/queue.rs` — fields: `buffer: Box<[UnsafeCell<MaybeUninit<T>>]>`, `capacity: usize`, `mask: usize`, `head: CachePadded<AtomicUsize>`, `tail: CachePadded<AtomicUsize>`, `sender_alive: AtomicBool`
- [X] T012 Implement `RingBuffer::new(capacity: usize)` — assert power-of-two, allocate slot array, initialize atomics
- [X] T013 Implement `RingBuffer::push(&self, value: T) -> bool` — non-blocking push, returns false if full. Include `// SAFETY:` comments for each unsafe block
- [X] T014 Implement `RingBuffer::pop(&self) -> Option<T>` — non-blocking pop, returns None if empty. Include `// SAFETY:` comments
- [X] T015 Implement `unsafe impl<T: Send> Send for RingBuffer<T>` and `unsafe impl<T: Send> Sync for RingBuffer<T>` with safety justification comments
- [X] T016 Add doc comments with runnable examples to all public items in `crates/component-core/src/channel/queue.rs`
- [X] T017 Verify all T007–T009 tests pass with `cargo test -p component-core`

**Checkpoint**: Lock-free SPSC ring buffer fully tested and functional. Foundation ready for user stories.

---

## Phase 3: User Story 1 — Actor Component Lifecycle (Priority: P1) 🎯 MVP

**Goal**: Define, activate, send messages to, and deactivate an actor component on its own thread.

**Independent Test**: Create a single actor, send it a message, verify it processes the message on a different thread, and verify clean shutdown.

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [X] T018 [P] [US1] Write unit tests for `ActorHandler` trait and `Actor` lifecycle in `crates/component-core/src/actor.rs` — test: new() creates idle actor, activate() returns handle, is_active() reflects state, deactivate() joins thread, double-activate returns AlreadyActive, double-deactivate returns NotActive
- [X] T019 [P] [US1] Write unit test for actor message processing in `crates/component-core/src/actor.rs` — test: send messages via handle, verify handler processes them on a different thread (compare thread::current().id()), verify sequential processing order
- [X] T020 [P] [US1] Write unit test for actor panic recovery in `crates/component-core/src/actor.rs` — test: handler panics on a specific message, error callback is invoked, actor continues processing subsequent messages
- [X] T021 [P] [US1] Write integration test for actor lifecycle in `crates/component-framework/tests/actor.rs` — test: full create→activate→send→deactivate cycle, verify on_start/on_stop callbacks, verify no resource leaks (thread joined)

### Implementation for User Story 1

- [X] T022 [US1] Implement `Sender<T>` and `Receiver<T>` wrapper structs in `crates/component-core/src/channel/mod.rs` — blocking send/recv using `thread::park`/`unpark`, `try_send`/`try_recv` non-blocking variants, `Clone` for `Sender<T>`, `Drop` for sender tracking (decrement count, signal closure)
- [X] T023 [US1] Implement `ActorHandler<M>` trait with `handle()`, `on_start()`, `on_stop()` default methods in `crates/component-core/src/actor.rs`
- [X] T024 [US1] Implement `Actor::new()` and `Actor::with_capacity()` constructors in `crates/component-core/src/actor.rs`
- [X] T025 [US1] Implement `Actor::activate()` — spawn thread, create inbound channel, run message loop with `catch_unwind` around handler, check shutdown `AtomicBool` after each message, return `ActorHandle<M>` in `crates/component-core/src/actor.rs`
- [X] T026 [US1] Implement `ActorHandle::send()`, `try_send()`, `deactivate()` in `crates/component-core/src/actor.rs` — deactivate sets shutdown flag, drops sender (closes channel), joins thread
- [X] T027 [US1] Implement `Actor::is_active()` in `crates/component-core/src/actor.rs`
- [X] T028 [US1] Add doc comments with runnable examples to all public actor types in `crates/component-core/src/actor.rs`
- [X] T029 [US1] Verify all T018–T021 tests pass with `cargo test --all`

**Checkpoint**: Actor lifecycle fully functional. Single actor can be created, activated, messaged, and deactivated.

---

## Phase 4: User Story 2 — Channel Components (SPSC and MPSC) (Priority: P1)

**Goal**: Connect actors using SPSC and MPSC channel components backed by lock-free queues.

**Independent Test**: Create SPSC/MPSC channels, bind sender/receiver, send messages, verify delivery.

### Tests for User Story 2

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [X] T030 [P] [US2] Write unit tests for `SpscChannel<T>` in `crates/component-core/src/channel/spsc.rs` — test: new() with capacity, sender()/receiver() return endpoints, send/recv delivers messages in order, 100K sequential messages with zero loss
- [X] T031 [P] [US2] Write unit tests for `MpscChannel<T>` in `crates/component-core/src/channel/mpsc.rs` — test: new() with capacity, multiple sender() calls succeed, receiver() returns endpoint, 8 producers × 10K messages with zero loss (SC-003)
- [X] T032 [P] [US2] Write unit test for channel closure in `crates/component-core/src/channel/spsc.rs` — test: drop all senders, receiver gets `Closed` after draining remaining messages
- [X] T033 [P] [US2] Write integration tests in `crates/component-framework/tests/channel_spsc.rs` — test: SPSC channel as component (IUnknown impl), introspection reports ISender/IReceiver, cross-thread send/recv
- [X] T034 [P] [US2] Write integration tests in `crates/component-framework/tests/channel_mpsc.rs` — test: MPSC channel as component, concurrent multi-producer delivery, closure signaling

### Implementation for User Story 2

- [X] T035 [US2] Implement MPSC ring buffer extension in `crates/component-core/src/channel/queue.rs` — ticket-based multi-producer write with `AtomicUsize` claim counter, sequential commit tracking
- [X] T036 [US2] Implement `SpscChannel<T>` struct in `crates/component-core/src/channel/spsc.rs` — `new(capacity)`, `with_default_capacity()`, `sender()`, `receiver()` with atomic bound tracking
- [X] T037 [US2] Implement `IUnknown` for `SpscChannel<T>` in `crates/component-core/src/channel/spsc.rs` — query_interface_raw returns ISender/IReceiver, provided_interfaces, version, introspection metadata
- [X] T038 [US2] Implement `MpscChannel<T>` struct in `crates/component-core/src/channel/mpsc.rs` — `new(capacity)`, `with_default_capacity()`, `sender()` (multi-call allowed), `receiver()`
- [X] T039 [US2] Implement `IUnknown` for `MpscChannel<T>` in `crates/component-core/src/channel/mpsc.rs` — same pattern as SpscChannel
- [X] T040 [US2] Add doc comments with runnable examples to all public channel types in `crates/component-core/src/channel/spsc.rs` and `crates/component-core/src/channel/mpsc.rs`
- [X] T041 [US2] Verify all T030–T034 tests pass with `cargo test --all`

**Checkpoint**: SPSC and MPSC channels fully functional as first-class components.

---

## Phase 5: User Story 3 — Binding Enforcement for Channel Topology (Priority: P2)

**Goal**: Enforce SPSC/MPSC topology constraints at bind time with descriptive errors.

**Independent Test**: Attempt over-binding on SPSC (rejected) and multi-binding on MPSC (accepted for senders, rejected for receivers).

### Tests for User Story 3

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [X] T042 [P] [US3] Write integration tests in `crates/component-framework/tests/binding_enforcement.rs` — test: SPSC rejects second sender with descriptive error, SPSC rejects second receiver, MPSC accepts multiple senders, MPSC rejects second receiver, sender disconnect frees slot for rebinding (FR-017)

### Implementation for User Story 3

- [X] T043 [US3] Implement binding rejection logic in `SpscChannel::sender()` and `SpscChannel::receiver()` in `crates/component-core/src/channel/spsc.rs` — return `ChannelError::BindingRejected` with descriptive reason when constraint violated
- [X] T044 [US3] Implement binding rejection for `MpscChannel::receiver()` in `crates/component-core/src/channel/mpsc.rs` — reject second receiver with descriptive error
- [X] T045 [US3] Implement sender slot release on `Sender<T>` drop in `crates/component-core/src/channel/mod.rs` — for SPSC, set `sender_bound` to false; for MPSC, decrement `sender_count`; when count reaches zero, signal closure
- [X] T046 [US3] Verify all T042 tests pass with `cargo test --all`

**Checkpoint**: Topology constraints enforced at bind time. Invalid bindings fail-fast with clear errors.

---

## Phase 6: User Story 4 — Actor-to-Actor Communication Pipeline (Priority: P2)

**Goal**: Assemble multi-stage actor pipelines using both first-party and third-party binding.

**Independent Test**: Register actor/channel factories, create 3-stage pipeline, send messages end-to-end, verify correct processing.

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [X] T047 [P] [US4] Write integration tests in `crates/component-framework/tests/actor_pipeline.rs` — test: two actors wired via first-party binding through SPSC channel, messages flow correctly; 3-stage pipeline (producer→processor→consumer) with sequential message transformation; third-party binding via registry with string names

### Implementation for User Story 4

- [X] T048 [US4] Ensure `SpscChannel<T>` and `MpscChannel<T>` work with `ComponentRef::from()` and `ComponentRegistry` in `crates/component-core/src/channel/spsc.rs` and `crates/component-core/src/channel/mpsc.rs` — implement necessary `From` conversions if not already covered by IUnknown
- [X] T049 [US4] Ensure `bind()` in `crates/component-core/src/binding.rs` correctly wires channel ISender/IReceiver to actor receptacles — verify `connect_receptacle_raw` delegates to channel endpoint binding
- [X] T050 [US4] Verify all T047 tests pass with `cargo test --all`

**Checkpoint**: Full actor + channel + binding pipeline working end-to-end.

---

## Phase 7: User Story 5 — Actor Component Examples (Priority: P3)

**Goal**: Provide runnable examples demonstrating actor components.

**Independent Test**: Each example compiles, runs, and produces expected output.

### Implementation for User Story 5

- [X] T051 [P] [US5] Create `examples/actor_ping_pong.rs` — two actors exchange a configurable number of messages through SPSC channels, print results
- [X] T052 [P] [US5] Create `examples/actor_pipeline.rs` — producer→processor→consumer pipeline using two SPSC channels, demonstrates message transformation
- [X] T053 [P] [US5] Create `examples/actor_fan_in.rs` — multiple producer actors send to a single consumer through an MPSC channel, demonstrates fan-in pattern
- [X] T054 [US5] Add `[[example]]` entries for all three new examples in `examples/Cargo.toml`
- [X] T055 [US5] Verify all three examples compile and run with `cargo run --example actor_ping_pong`, `cargo run --example actor_pipeline`, `cargo run --example actor_fan_in`

**Checkpoint**: All three examples compile, run, and produce correct output.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Benchmarks, documentation, CI gate validation

- [X] T056 [P] Create Criterion benchmark `crates/component-framework/benches/channel_throughput.rs` — SPSC throughput (single-thread push/pop cycle, 1M messages), MPSC throughput (8 producers, 1 consumer), measure messages/sec
- [X] T057 [P] Create Criterion benchmark `crates/component-framework/benches/actor_latency.rs` — measure round-trip actor message latency (send→handle→respond)
- [X] T058 Register new benchmarks in `crates/component-framework/Cargo.toml` under `[[bench]]` sections
- [X] T059 Run `cargo fmt --check` and fix any formatting issues
- [X] T060 Run `cargo clippy -- -D warnings` and fix any lint warnings
- [X] T061 Run `cargo doc --no-deps` and fix any documentation warnings
- [X] T062 Run `cargo test --all` and verify zero failures (full regression)
- [X] T063 Run `cargo bench` and verify benchmarks execute successfully
- [X] T064 Run full CI gate: `cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps`

**Checkpoint**: All code clean, all tests pass, benchmarks run, CI gate green.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Phase 2 — actor lifecycle needs Sender/Receiver from ring buffer
- **US2 (Phase 4)**: Depends on Phase 2 — channel components wrap ring buffer
- **US3 (Phase 5)**: Depends on US2 (Phase 4) — binding enforcement extends channel components
- **US4 (Phase 6)**: Depends on US1 + US2 + US3 — pipeline requires actors, channels, and enforcement
- **US5 (Phase 7)**: Depends on US1 + US2 + US3 — examples demonstrate complete feature set
- **Polish (Phase 8)**: Depends on all user stories

### User Story Dependencies

- **US1 (Actor Lifecycle)**: Depends on Foundational only. Needs `Sender<T>` and `Receiver<T>` (implemented in US1 phase since they wrap the ring buffer).
- **US2 (Channel Components)**: Depends on Foundational only. Can run in parallel with US1.
- **US3 (Binding Enforcement)**: Depends on US2 (channel components must exist before constraining binds).
- **US4 (Pipeline)**: Depends on US1 + US2 + US3 (requires actors, channels, and enforcement).
- **US5 (Examples)**: Depends on US1 + US2 + US3 (examples use all features).

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Error types before structs
- Structs before methods
- Core logic before IUnknown integration
- Unit tests before integration tests
- Story complete before moving to next priority

### Parallel Opportunities

- **Phase 1**: T002, T003, T004 can run in parallel (different files)
- **Phase 2**: T008, T009 can run in parallel with each other (after T007)
- **Phase 3**: T018, T019, T020, T021 test tasks can run in parallel
- **Phase 4**: T030, T031, T032, T033, T034 test tasks can run in parallel
- **Phase 4**: US1 and US2 can run in parallel after Phase 2
- **Phase 7**: T051, T052, T053 example tasks can run in parallel

---

## Parallel Example: User Story 2 (Channels)

```bash
# Launch all channel tests in parallel (different files):
Task: T030 "Unit tests for SpscChannel in channel/spsc.rs"
Task: T031 "Unit tests for MpscChannel in channel/mpsc.rs"
Task: T032 "Unit test for channel closure in channel/spsc.rs"
Task: T033 "Integration tests in tests/channel_spsc.rs"
Task: T034 "Integration tests in tests/channel_mpsc.rs"

# After tests fail, implement in order:
Task: T035 "MPSC ring buffer extension in channel/queue.rs"
Task: T036 "SpscChannel struct in channel/spsc.rs"
Task: T037 "IUnknown for SpscChannel in channel/spsc.rs"
Task: T038 "MpscChannel struct in channel/mpsc.rs"
Task: T039 "IUnknown for MpscChannel in channel/mpsc.rs"
```

---

## Implementation Strategy

### MVP First (US1 + US2 Only)

1. Complete Phase 1: Setup (module scaffolding)
2. Complete Phase 2: Foundational (lock-free ring buffer)
3. Complete Phase 3: US1 (actor lifecycle) — in parallel with Phase 4
4. Complete Phase 4: US2 (channel components) — in parallel with Phase 3
5. **STOP and VALIDATE**: Actors can send/receive messages through channels
6. This is a functional MVP even without binding enforcement or examples

### Incremental Delivery

1. Setup + Foundational → Ring buffer working
2. US1 + US2 → Actors + Channels working (MVP!)
3. US3 → Topology enforcement added
4. US4 → Pipeline integration validated
5. US5 → Examples for documentation
6. Polish → Benchmarks, CI gate, docs

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Constitution mandates TDD: tests first, then implement
- Each `unsafe` block in queue.rs MUST have a `// SAFETY:` comment
- Default channel capacity: 1024 (power of two)
- Error callback required at actor creation time (FR-006)
- Commit after each task or logical group
- Stop at any checkpoint to validate independently
