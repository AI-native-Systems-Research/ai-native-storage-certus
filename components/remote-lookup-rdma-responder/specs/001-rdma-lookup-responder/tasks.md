---
description: "Task list for RDMA Lookup Responder implementation"
---

# Tasks: RDMA Lookup Responder

**Input**: Design documents from `specs/001-rdma-lookup-responder/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: INCLUDED — the spec explicitly requires unit tests (FR-015) and a
Criterion telemetry-overhead benchmark (SC-006), so test tasks are first-class.

**Organization**: Grouped by user story (spec.md priorities P1–P3). The MVP is the
three P1 stories delivered over the **mock CM seam** (no RDMA hardware); the real
`rdma_cm` accept path is a clearly-separated hardware follow-up phase (research D9).

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files / no dependency on incomplete tasks)
- **[Story]**: US1–US6 map to the spec's user stories
- Paths are relative to `components/remote-lookup-rdma-responder/` unless noted

## Path Conventions

Single Rust component crate. Source in `src/`, benches in `benches/`, unit tests
live in-module (`#[cfg(test)] mod tests`) mirroring the sibling
`remote-lookup-rdma-initiator`. Interface lives in the `interfaces` crate.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Crate scaffolding, features, and module layout (no hardware linkage yet).

- [X] T001 Add `[features] default = []` / `telemetry = []`, `[dev-dependencies] criterion` (html_reports), and a `[[bench]] name = "connection_telemetry", harness = false` stanza to `Cargo.toml`
- [X] T002 [P] Declare `mod connection;` and `mod telemetry;` (and `#[cfg(test)] mod loopback_test;` gated for later) in `src/lib.rs`, re-exporting the public seam/telemetry types for benches
- [X] T003 [P] Verify the crate stays excluded from `default-members` in the workspace root `Cargo.toml` (links rdma-core later, like the initiator)

**Checkpoint**: `cargo build -p remote-lookup-rdma-responder` compiles with empty module stubs.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: The mock CM seam, connection table, and telemetry collector that every P1/P2/P3 story builds on.

**⚠️ CRITICAL**: No user-story work can begin until this phase is complete.

- [X] T004 [P] Implement the feature-gated `TelemetryCollector` in `src/telemetry.rs`: a ZST no-op with `#[allow(clippy::unused_self)]` when the `telemetry` feature is off, `AtomicU64` counters when on, identical method surface (metrics: connections accepted, identified, unidentified, teardowns, accept-loop errors) — mirror `remote-lookup-rdma-initiator/src/telemetry.rs`
- [X] T005 Define the CM seam in `src/connection.rs`: traits `CmListener` (bind/listen + `wait() -> Vec<CmEvent>`) and `CmConnection` (carries one QP; `to_error()` + drop-destroy), and the internal `CmEvent` enum (`ConnectRequest { private_data: Option<Vec<u8>>, conn }`, `Command(ResponderCommand)`, `Stop`)
- [X] T006 [P] Implement `MockCmSeam` + `MockCmConn` in `src/connection.rs` (depends T005): inject connect events with arbitrary `private_data`, deliver commands on an event-driven wake with no pending connect, and record QP→ERROR ordering for assertions
- [X] T007 Implement `ConnectionTable` + `ConnState` (`Active → Draining → Dead`) in `src/connection.rs` (depends T005): `HashMap<PeerId, ConnectionEntry>` plus an unidentified side-list for `node: None` entries
- [X] T008 Refactor `run_accept_loop` in `src/lib.rs` to drive the `CmListener` seam instead of the `try_recv` + `sleep` placeholder, routing `CmEvent`s to the `ConnectionTable` and emitting `ResponderEvent`s (depends T005, T007)
- [X] T008a Implement **lossless** `ResponderEvent` delivery in `src/lib.rs` (FR-011a): replace the skeleton's fire-and-forget `let _ = event_tx.send(...)` with a backpressure send (block/retry until enqueued) so no event — especially `DisconnectAck` — is ever dropped on a full channel; add a unit test that fills the event channel and asserts a subsequent `DisconnectAck` is delivered (not dropped, not an error) once the consumer drains (depends T008)

**Checkpoint**: Foundation ready — the actor drives events through the mock seam with lossless delivery; user stories can proceed.

---

## Phase 3: User Story 1 - Bind and advertise a listening endpoint (Priority: P1) 🎯 MVP

**Goal**: The mainline supplies the RoCE IPv4, `initialize()` starts the (seam-backed) accept loop, and `local_endpoint()` advertises `{ip, port}`.

**Independent Test**: `local_endpoint()` → `NotInitialized` before init; after `set_bind_ip()` + `initialize()`, returns the supplied IP with a non-placeholder port; missing IP → `Bind` (quickstart scenarios 1, 1b).

- [X] T009 [US1] `set_bind_ip(ip)` stores the IP (overriding auto-detection) and `initialize()` defers to auto-detect (first active device) when unset — implemented in `src/lib.rs` / `src/rdma.rs` per FR-002a
- [X] T010 [US1] Wire `initialize()` to bind/listen via the `CmListener` seam and publish `Endpoint { ip, port }`; `local_endpoint()` returns it or `NotInitialized`; second `initialize()` → `AlreadyInitialized` (loop undisturbed) in `src/lib.rs`
- [X] T011 [P] [US1] Unit tests in `src/lib.rs`: endpoint before/after init (SC-001), unset bind IP → accepted (defers to auto-detect), double-init → `AlreadyInitialized`, `open_control_channel()` before init → `NotInitialized` and a **second `open_control_channel()` → `ChannelClosed`** (FR-011), and two component instances expose independent endpoints (mock-level SC-004 sanity; true ephemeral-port distinctness is hardware-only, see T030) — *extend the existing three tests*

**Checkpoint**: US1 functional and independently testable over the mock seam.

---

## Phase 4: User Story 2 - Accept connections and correlate identity (Priority: P1)

**Goal**: On `CONNECT_REQUEST`, read the zyre UUID from `private_data`, key the table by `PeerId`, accept, and emit `ConnectionEstablished { node }`.

**Independent Test**: A connect with a known UUID → `ConnectionEstablished { node: Some(peer) }`; empty/malformed `private_data` → `node: None` (SC-005); a second connect for an `Active` peer doesn't corrupt the entry.

- [X] T012 [US2] Handle `CmEvent::ConnectRequest` in `src/connection.rs` + `src/lib.rs`: parse `private_data` → `PeerId`, insert an `Active` entry (or push to the unidentified side-list when absent/malformed), and emit `ConnectionEstablished { node }` (FR-005/FR-006)
- [X] T013 [US2] Handle a second connect for an already-`Active` peer without corrupting the existing entry's state (Story 2.3) in `src/connection.rs`
- [X] T014 [P] [US2] Unit tests in `src/connection.rs`: valid UUID → `Some(peer)`, empty `private_data` → `None`, second-connect-for-Active (SC-005)

**Checkpoint**: US1 + US2 both work independently.

---

## Phase 5: User Story 3 - Teardown before reclaim (Priority: P1)

**Goal**: `Disconnect { node }` drives the QP to ERROR **before** emitting the single `DisconnectAck { node }`; idempotent; teardown not raced by new work. This is the memory-safety linchpin.

**Independent Test**: `Disconnect` on an `Active` node transitions QP→ERROR observably before the ack and ends `Dead`; `Disconnect` on unknown/`Dead` → one idempotent ack; a connect for a `Draining` node is refused (SC-002).

- [X] T015 [US3] Add the asserted `CmConnection::to_error()` (QP→ERROR) to the seam and its `MockCmConn` impl (records order relative to the ack) in `src/connection.rs`
- [X] T016 [US3] Implement the `Disconnect { node }` handler in `src/connection.rs` + `src/lib.rs` (depends T015): `Active → Draining`, `to_error()` (assert — fail-stop on failure), destroy QP (best-effort, log via `ILogger`), `→ Dead`, then emit `DisconnectAck`; idempotent for unknown/`Dead`; refuse (`reject`) a new connect while `Draining` (FR-007/FR-008)
- [X] T017 [P] [US3] Unit tests in `src/connection.rs`: ERROR-before-ack ordering (SC-002), idempotent disconnect, refuse-while-draining

**Checkpoint**: All three P1 stories complete — MVP over the mock seam.

---

## Phase 6: User Story 4 - Prompt command servicing (Priority: P2)

**Goal**: The wait multiplexes `{cm fd, command inbox, stop}` so a `Disconnect` is serviced on an event-driven wake, never behind a pending/absent accept.

**Independent Test**: Inject a `Disconnect` with zero pending connections and assert the ack is delivered on an event-driven wake — no intervening connection event, no poll cycle (SC-003, structural).

- [X] T018 [US4] Design the event-driven wait in `src/connection.rs`: pair the command inbox and stop with `eventfd`s (written by the sender / `signal_stop`), and define the `CmListener::wait()` contract as an `epoll` multiplex over `{cm fd, command eventfd, stop eventfd}`; the `MockCmSeam` wakes on a command with no connect
- [X] T019 [P] [US4] Structural test (SC-003) in `src/connection.rs`: enqueue a `Disconnect` with no pending connects and assert the ack arrives via the event-driven wake (no connection event, no sleep/poll interval)

**Checkpoint**: US4 verified over the mock seam.

---

## Phase 7: User Story 5 - Lifecycle and NUMA placement (Priority: P2)

**Goal**: Pin the accept-loop thread to the instance's NUMA node; stop cooperatively; shut down cleanly tearing down all connections and the listener.

**Independent Test**: `set_actor_cpu(n)` before `initialize()` pins the thread; `shutdown()` joins + is idempotent; `signal_stop()` exits the loop without join.

- [X] T020 [US5] As the accept-loop thread's first action, pin via `component_core::numa::CpuSet::from_cpu(cpu)` + `set_thread_affinity` when `actor_cpu` is set (best-effort; log on failure) in `src/lib.rs` (FR-012)
- [X] T021 [US5] Extend `shutdown()` in `src/lib.rs` to tear down all remaining connections (identified + unidentified) and the listener after joining, keep it idempotent; keep `signal_stop()` a cooperative no-join exit (FR-013)
- [X] T022 [P] [US5] Unit tests in `src/lib.rs`: `shutdown()` joins + second call is a no-op; `signal_stop()` exits the loop; `set_actor_cpu` honored; and an initialize → disconnect → shutdown cycle succeeds with **no `logger` receptacle bound** (FR-014 — a missing logger is never an error)

**Checkpoint**: US1–US5 independently functional.

---

## Phase 8: User Story 6 - Operator telemetry (Priority: P3)

**Goal**: With `--features telemetry`, connection/teardown metrics are recorded; disabled, the collector is a ZST no-op. Overhead < 5% (SC-006).

**Independent Test**: Build with `--features telemetry`, drive accept/disconnect over the mock seam, read the counters; feature-off build incurs no call-site cost.

- [X] T023 [US6] Wire `TelemetryCollector` call sites (depends T004): connections accepted/identified/unidentified (US2 path), teardowns/disconnect-acks (US3 path), accept-loop errors, in `src/connection.rs` + `src/lib.rs`
- [X] T024 [P] [US6] Telemetry unit tests behind `#[cfg(feature = "telemetry")]` in `src/telemetry.rs` (and a wiring test in `src/connection.rs`) asserting each counter advances
- [X] T025 [US6] Criterion benchmark `benches/connection_telemetry.rs` (depends T006) driving the accept/disconnect path over a bench-local mock seam; document the two-run `--save-baseline off` / `--baseline off` workflow (SC-006), mirroring `push_telemetry.rs`

**Checkpoint**: All six stories complete over the mock seam (full CI-testable MVP+).

---

## Phase 9: Hardware Follow-Up (deferred; hardware-gated) 🔌

**Purpose**: The production `rdma_cm` accept path behind the seam — verified on RDMA hardware, not in CI (research D9). Only the `#[ignore]` loopback test exercises it.

- [X] T026 [P] Add responder-side raw bindings to `src/ffi.rs`: `rdma_bind_addr`, `rdma_listen`, `rdma_get_src_port`, `rdma_accept`, `rdma_reject`, `rdma_get_cm_event`, `rdma_ack_cm_event`, `ibv_modify_qp` (+ `IBV_QP_STATE`), `rdma_destroy_qp`, and the needed structs — mirror the initiator's `ffi.rs`
- [X] T027 [P] Add `src/wrapper.c` C shim for inline ibverbs (e.g. an `ibv_modify_qp` helper for the QP→ERROR transition)
- [X] T028 Add `build.rs` (depends T027): `pkg_config::probe_library` for `libibverbs`/`librdmacm`, compile `src/wrapper.c` via `cc` — mirror the initiator's `build.rs`
- [X] T029 Implement `RealCmSeam` / `RealCmConn` in `src/rdma.rs` (depends T026, T028): bind port 0 on the supplied IP, `rdma_listen`, `rdma_get_src_port`, `epoll` over `{cm fd, command eventfd, stop eventfd}`, read `private_data`, `rdma_accept`/`rdma_reject`, and the asserted QP→ERROR + best-effort destroy (all `unsafe` blocks carry `// SAFETY:`)
- [X] T030 [P] Add the hardware-gated `#[ignore]` loopback test in `src/loopback_test.rs` (depends T029): real accept + UUID correlation + teardown-before-ack, and two co-resident listeners binding distinct ephemeral ports on one NIC (SC-004 — the *authoritative* validation; the mock-level check in T011 only asserts endpoint independence, since real port distinctness requires a NIC)

**Checkpoint**: Real accept path validated on hardware via `cargo test -- --ignored loopback`.

---

## Phase 10: Polish & Cross-Cutting Concerns

- [X] T031 [P] Write `README.md` for the component (role, interfaces, receptacles, non-default-member note), mirroring the initiator's
- [X] T032 [P] Write `info/DESIGN.md` (referenced by `src/lib.rs`) capturing the seam, state machine, and teardown ordering
- [X] T033 [P] Ensure `cargo doc -p remote-lookup-rdma-responder --no-deps` is warning-free; add runnable doc examples to public items
- [X] T034 Run `cargo fmt --check` and `cargo clippy -p remote-lookup-rdma-responder --all-features -- -D warnings`
- [X] T035 Run the `quickstart.md` validation scenarios and `/component-check-leakage` (confirm `remote-lookup` reaches the responder only via its interfaces + control channel) — leakage check clean (0 violations); quickstart logic scenarios covered by `cargo test`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: no dependencies.
- **Foundational (Phase 2)**: depends on Setup — BLOCKS all user stories.
- **User Stories (Phases 3–8)**: depend on Foundational. P1 stories (US1→US2→US3) are the MVP; US4/US5 (P2) and US6 (P3) follow. Stories are independently testable but share `src/connection.rs`/`src/lib.rs`, so parallelism across them is limited (see below).
- **Hardware Follow-Up (Phase 9)**: depends on the seam contract (T005) and is otherwise independent of the mock-path stories; do it after the MVP.
- **Polish (Phase 10)**: after the stories you intend to ship.

### User Story Dependencies

- **US1 (P1)**: after Foundational. Independent.
- **US2 (P1)**: after Foundational. Independent (adds the connect path).
- **US3 (P1)**: after Foundational; conceptually builds on an accepted connection (US2) but its teardown logic is testable on a table entry created directly.
- **US4 (P2)**: after Foundational; refines the wait added in T008.
- **US5 (P2)**: after Foundational; touches lifecycle in `src/lib.rs`.
- **US6 (P3)**: after Foundational (T004); call sites depend on US2/US3 existing.

### Parallel Opportunities

- Setup: T002, T003 in parallel.
- Foundational: T004 (telemetry.rs) ∥ T005 (seam traits); then T006 ∥ T007 once T005 lands.
- Within a story, the `[P]` test task touches a test module and can be written alongside impl.
- Hardware phase: T026 ∥ T027 (then T028), and T030 after T029.
- Polish: T031 ∥ T032 ∥ T033.
- Note: US1–US6 mostly edit the same two files (`src/lib.rs`, `src/connection.rs`), so run them **sequentially** in priority order rather than in parallel to avoid conflicts.

---

## Parallel Example: Foundational Phase

```bash
# Independent files — safe to run together:
Task: "Implement feature-gated TelemetryCollector in src/telemetry.rs"   # T004
Task: "Define CmListener/CmConnection/CmEvent seam in src/connection.rs"  # T005
# After T005:
Task: "Implement MockCmSeam + MockCmConn in src/connection.rs"            # T006
Task: "Implement ConnectionTable + ConnState in src/connection.rs"        # T007
```

---

## Implementation Strategy

### MVP First (three P1 stories over the mock seam)

1. Phase 1 Setup → Phase 2 Foundational (mock seam + table + telemetry ZST).
2. US1 (bind/advertise) → US2 (accept/correlate) → US3 (teardown-before-reclaim).
3. **STOP and VALIDATE**: `cargo test -p remote-lookup-rdma-responder` — the safety-critical SC-002 ordering test is the gate.

### Incremental Delivery

1. MVP (US1–US3) → US4 (prompt servicing) → US5 (lifecycle/NUMA) → US6 (telemetry + bench).
2. Then Phase 9 hardware follow-up (real `rdma_cm` path behind the seam), validated by the `#[ignore]` loopback test on an RDMA box.
3. Phase 10 polish (README, DESIGN, docs, clippy, quickstart, leakage check).

---

## Notes

- `[P]` = different files, no incomplete-task dependency.
- Tests are in-module (`#[cfg(test)] mod tests`) per the sibling component's style; the benchmark lives in `benches/`.
- All rdma-core FFI (`unsafe`) requires `// SAFETY:` comments (CLAUDE.md).
- The mock-seam MVP (Phases 1–8) is fully CI-testable without RDMA hardware; only Phase 9 needs a NIC.
- Commit after each task or logical group; stop at any checkpoint to validate a story independently.
