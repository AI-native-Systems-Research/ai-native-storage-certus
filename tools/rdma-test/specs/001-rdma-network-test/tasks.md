# Tasks: RDMA Network Test Tool

**Input**: Design documents from `/specs/001-rdma-network-test/`

**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization, build system, and manual FFI binding foundation

- [X] T001 Create Cargo.toml with workspace, build deps (pkg-config), runtime deps (tokio, clap, anyhow, tracing, serde, serde_json) in Cargo.toml
- [X] T002 Create build.rs to link libibverbs and librdmacm via pkg-config in build.rs
- [X] T003 [P] Create manual FFI bindings for libibverbs types and functions in src/ffi.rs
- [X] T004 [P] Create manual FFI bindings for librdmacm types and functions in src/ffi.rs (extend same file as T003)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Safe RDMA wrapper layer and statistics module that ALL user stories depend on

**CRITICAL**: No user story work can begin until this phase is complete

- [X] T005 Implement safe RDMA context wrapper (device open, PD alloc, CQ create, MR register/deregister with RAII Drop) in src/rdma.rs
- [X] T006 Implement RDMA connection management wrapper (rdmacm event loop, resolve_addr, resolve_route, connect, accept, listen, disconnect) in src/rdma.rs
- [X] T007 Implement QP creation and state transition helpers (RESET→INIT→RTR→RTS) in src/rdma.rs
- [X] T008 [P] Implement post_send, post_recv, and poll_cq safe wrappers with retry logic (3 retries per spec) in src/rdma.rs
- [X] T009 [P] Implement statistics computation module (min, max, mean, median, P95, P99, stddev) in src/stats.rs
- [X] T010 [P] Implement output formatting module with human-readable and JSON serialization (serde) in src/output.rs
- [X] T011 Implement CLI argument parsing with clap derive (server/client subcommands, all global options per contracts/cli-interface.md) in src/main.rs

**Checkpoint**: Foundation ready — RDMA connection lifecycle, stats, and output formatting are available for all stories

---

## Phase 3: User Story 1 - Measure RDMA Throughput (Priority: P1) MVP

**Goal**: Perform RDMA Write throughput measurement between two nodes and report bandwidth, message rate, and total data

**Independent Test**: Launch server on one RDMA host, client on another with `--test write`, verify bandwidth results are produced

### Implementation for User Story 1

- [X] T012 [US1] Implement server-side throughput handler (allocate MR, send MR info to client via Send, wait for completion signal) in src/server.rs
- [X] T013 [US1] Implement client-side throughput benchmark (receive remote MR info, warmup RDMA Writes, timed RDMA Write loop, signal completion) in src/throughput.rs
- [X] T014 [US1] Implement MR info exchange protocol (RemoteMrInfo struct serialization via Send/Recv during setup) in src/rdma.rs
- [X] T015 [US1] Wire throughput test into client dispatch (connect, run throughput, print/json results, disconnect) in src/client.rs
- [X] T016 [US1] Wire throughput test into server dispatch (listen, accept, handle throughput, exit) in src/server.rs
- [X] T017 [US1] Add ThroughputResult to output module with both human and JSON formatting in src/output.rs

**Checkpoint**: User Story 1 fully functional — `rdma-test server -t throughput` + `rdma-test client -a <ip> -t throughput` produces bandwidth results

---

## Phase 4: User Story 2 - Measure RDMA Latency and Jitter (Priority: P1)

**Goal**: Perform Send/Recv ping-pong latency measurement and report statistical distribution including jitter

**Independent Test**: Launch server on one host, client on another with `--test latency`, verify min/max/mean/p95/p99/jitter output

### Implementation for User Story 2

- [X] T018 [US2] Implement server-side latency handler (receive message, echo back via Send, loop for warmup + iterations) in src/server.rs
- [X] T019 [US2] Implement client-side latency benchmark (warmup loop, timed Send/Recv ping-pong, collect RTT/2 samples) in src/latency.rs
- [X] T020 [US2] Wire latency test into client dispatch (connect, run latency, print/json results, disconnect) in src/client.rs
- [X] T021 [US2] Wire latency test into server dispatch (accept, handle latency, exit) in src/server.rs
- [X] T022 [US2] Add LatencyResult to output module with both human and JSON formatting in src/output.rs
- [X] T023 [US2] Implement "all" test mode (run throughput then latency sequentially on same connection) in src/client.rs and src/server.rs

**Checkpoint**: User Story 2 fully functional — latency test produces statistical results independently

---

## Phase 5: User Story 3 - Launch Tests Remotely via SSH Script (Priority: P2)

**Goal**: Single-command launch of client/server pair across two hosts via SSH

**Independent Test**: Provide two SSH-accessible hosts, run launch script, verify server starts, client runs, results displayed, cleanup occurs

### Implementation for User Story 3

- [X] T024 [US3] Create SSH launch script with server/client host args, option forwarding, and cleanup trap in scripts/launch.sh
- [X] T025 [US3] Implement server health check (detect premature exit before launching client) in scripts/launch.sh
- [X] T026 [US3] Implement result collection (capture client stdout, display on local terminal) in scripts/launch.sh
- [X] T027 [US3] Support RDMA_TEST_BIN, RDMA_TEST_PORT, RDMA_TEST_STARTUP_DELAY environment variables in scripts/launch.sh

**Checkpoint**: SSH launch script fully functional — single command runs full test between two remote nodes

---

## Phase 6: User Story 4 - Verify RDMA/ibverbs Availability (Priority: P2)

**Goal**: Check ibverbs library and device availability at startup with actionable diagnostics

**Independent Test**: Run tool on machine without RDMA, verify informative error; run on machine with RDMA, verify device listing

### Implementation for User Story 4

- [X] T028 [US4] Implement libibverbs presence check (probe library paths and /sys/class/infiniband) in src/main.rs
- [X] T029 [US4] Implement device enumeration (list devices, query port attributes, detect IB vs RoCE from link_layer) in src/main.rs
- [X] T030 [US4] Implement diagnostic messages (missing library → install instructions, no devices → driver/SoftRoCE suggestions) in src/main.rs
- [X] T031 [US4] Gate all RDMA operations behind successful availability check (exit code 1 on failure) in src/main.rs

**Checkpoint**: Tool gracefully handles missing RDMA with actionable error messages

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Improvements that affect multiple user stories

- [X] T032 [P] Implement partial results reporting (track completed iterations, report on failure after retries exhausted) in src/client.rs
- [X] T033 [P] Add input validation (reject size=0, iterations=0, invalid addresses) in src/main.rs
- [X] T034 [P] Add tracing/logging with RUST_LOG env var support for debug output in src/main.rs
- [X] T035 Run cargo clippy -- -D warnings and fix all warnings across all source files
- [ ] T036 Validate quickstart.md scenarios work end-to-end on RDMA-capable hardware

**Note (spec-sync, 2026-07-22)**: T032 is checked off as complete, but the drift report found that connection-level retry and `partial: true` reporting (FR-012) are not actually implemented in the current code (`src/rdma.rs`, `src/client.rs`, `src/main.rs`). This is tracked as an align task, not re-opened here — see `.specify/sync/align-tasks.md`.

---

## Phase 8: Backfilled Bandwidth Test Variants (Read/Send/Recv) [spec-sync backfill]

**Purpose**: These tasks document already-implemented functionality (RDMA Read, Send, and Recv bandwidth tests) discovered during spec-sync drift analysis (2026-07-22) and newly captured as FR-014/FR-015/FR-016/FR-017 in `spec.md`. No code changes were made; this phase exists for traceability only.

- [X] T037 [P] RDMA Read one-sided throughput benchmark (`run_read_client`) in src/throughput.rs, wired in src/client.rs and src/server.rs
- [X] T038 [P] Two-sided Send bandwidth benchmark in src/send_bw.rs, wired in src/client.rs and src/server.rs
- [X] T039 [P] Two-sided Recv bandwidth benchmark in src/recv_bw.rs, wired in src/client.rs and src/server.rs

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Foundational (Phase 2)
- **User Story 2 (Phase 4)**: Depends on Foundational (Phase 2); independent of US1
- **User Story 3 (Phase 5)**: Depends on US1 or US2 being complete (needs a working binary)
- **User Story 4 (Phase 6)**: Depends on Setup only (uses FFI directly, no RDMA wrapper needed)
- **Polish (Phase 7)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: After Foundational — fully independent
- **User Story 2 (P1)**: After Foundational — fully independent of US1 (can run in parallel)
- **User Story 3 (P2)**: After US1 or US2 — needs working binary to launch remotely
- **User Story 4 (P2)**: After Setup — uses only FFI layer, no RDMA wrapper needed (can be done in parallel with Foundational)

### Within Each User Story

- Server-side handler before client-side benchmark logic
- Wire into dispatch after both sides implemented
- Output formatting last (depends on result types being defined)

### Parallel Opportunities

- T003 + T004: FFI bindings for ibverbs and rdmacm (same file but independent sections)
- T008 + T009 + T010: Post operations, stats module, and output module are independent
- US1 + US2: Can be implemented in parallel after Foundational
- US4: Can start after Phase 1 (independent of Foundational phase)
- T032 + T033 + T034: All Polish tasks are independent

---

## Parallel Example: User Stories 1 & 2

```text
# After Foundational phase completes, both stories can start simultaneously:

# Developer A: User Story 1 (Throughput)
Task T012: Server-side throughput handler in src/server.rs
Task T013: Client-side throughput benchmark in src/throughput.rs
Task T014: MR info exchange protocol in src/rdma.rs
Task T015: Wire into client dispatch in src/client.rs
Task T016: Wire into server dispatch in src/server.rs
Task T017: Output formatting in src/output.rs

# Developer B: User Story 2 (Latency)
Task T018: Server-side latency handler in src/server.rs
Task T019: Client-side latency benchmark in src/latency.rs
Task T020: Wire into client dispatch in src/client.rs
Task T021: Wire into server dispatch in src/server.rs
Task T022: Output formatting in src/output.rs
Task T023: "All" test mode in src/client.rs + src/server.rs
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001-T004)
2. Complete Phase 2: Foundational (T005-T011)
3. Complete Phase 3: User Story 1 — Throughput (T012-T017)
4. **STOP and VALIDATE**: Test throughput measurement between two RDMA nodes
5. Deploy if ready — users can measure throughput immediately

### Incremental Delivery

1. Setup + Foundational → RDMA infrastructure ready
2. Add User Story 1 → Throughput works independently → Deploy (MVP!)
3. Add User Story 2 → Latency works independently → Deploy
4. Add User Story 4 → Graceful failure on non-RDMA machines → Deploy
5. Add User Story 3 → SSH automation → Deploy
6. Polish → Partial results, validation, logging → Final release

### Single Developer Strategy

1. Phase 1 + Phase 2: Foundation (~40% of effort)
2. Phase 3: Throughput (MVP, ~20%)
3. Phase 4: Latency (~20%)
4. Phase 6: Device check (~10%, can interleave earlier)
5. Phase 5: SSH script (~5%)
6. Phase 7: Polish (~5%)

---

## Notes

- [P] tasks = different files, no dependencies on incomplete tasks
- [Story] label maps task to specific user story for traceability
- src/server.rs and src/client.rs are touched by multiple stories — coordinate if parallelizing
- All RDMA data-path operations use synchronous CQ polling (no async) for measurement accuracy
- The "all" test mode (T023) is the only cross-story integration point
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
