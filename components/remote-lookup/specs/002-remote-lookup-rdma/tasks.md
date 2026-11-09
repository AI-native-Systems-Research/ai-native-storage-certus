---

description: "Task list for 002-remote-lookup-rdma"
---

# Tasks: Remote Lookup over Zyre + RDMA

**Input**: Design documents from `components/remote-lookup/specs/002-remote-lookup-rdma/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: INCLUDED — the feature specifies a two-layer test strategy (research Decision 8): an
in-process **multi-node (N ≥ 4) mesh over real zyre** (gossip on TCP loopback) with the NIC and
local state mocked, driven by a reusable `TestMesh` fixture in `tests/mesh.rs`; plus `#[ignore]`
hardware loopback tests that swap the mock RDMA seams for the real crates. Tests assert
timing-robust invariants (never exact reply-order-dependent routing); determinism comes from a
discovery barrier + app-level reply delays sized above transport jitter (research Decision 8).

**Organization**: Tasks are grouped by user story. The core MVP is memory-tier hits:
US1 + US2 + US3 + US6 + US7 (all ship together — the protocol is symmetric, so a node must both
ask and answer to be demonstrable). **US4 (disk fallback) and US5 (retry) are now in scope** (no
longer deferred): US5 is self-contained in the actor, and US4's cross-component dependency **D3**
is satisfied by an existing interface method. Delivery stays incremental — get the memory path
green first, then layer US5, then US4.

**Cross-component prerequisites**: D1 **dropped** (publish-on-success, research Decision 5 — no
dispatch-map change). D2 **done** (initiator PeerId stamping, commit `e77c4a5`). D3 **resolved** —
`IDispatcher::promote_to_memory_tier` already exists (idispatcher.rs:657, implemented by dispatcher
+ dispatcher-p2p); remote-lookup only adds a `dispatcher: IDispatcher` receptacle. The **one**
`interfaces`-crate change is adding `IRemoteLookup::initialize(LookupConfig)` + a public
`LookupConfig` (T001a, its own commit). **D4 (SPDK gating, research Decision 10)**: the consumed
`IDispatchMap`/`IDispatcher` trait defs are `spdk`-gated in `interfaces`; short-term resolution is to
enable `interfaces/spdk` in remote-lookup's Cargo.toml and remove remote-lookup from the workspace
`default-members` (done — Cargo-manifest/workspace edit, no `interfaces` source change). Future work:
ungate those trait defs (like `IMemoryTier` in `db7f70a`) so remote-lookup can drop the feature and
rejoin `default-members`. One wiring contract applies: the mainline must `Receptacle::disconnect()`
one side of the `dispatcher-p2p ⇄ remote-lookup` Arc cycle at teardown (research Decision 7).

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story the task belongs to

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Crate wiring and module skeleton so all later work compiles incrementally.

- [X] T001a **[interfaces crate — own commit]** Add `initialize(&self, config: LookupConfig) ->
  Result<(), RemoteLookupError>` to the `IRemoteLookup` `define_interface!` block and define a public
  `LookupConfig` struct (derives `Default`, fields per data-model.md) in
  `components/interfaces/src/iremote_lookup.rs`. Update the sole existing impl
  (`components/remote-lookup/src/lib.rs` placeholder) to satisfy the trait. Mirrors
  `IDispatcher::initialize(DispatcherConfig)` (FR-001/FR-022). Verify `cargo build -p interfaces`
  and `cargo build -p remote-lookup`.
- [X] T001 Update `components/remote-lookup/Cargo.toml`: add workspace deps for the new receptacles
  (`interfaces` already present — confirm it exposes `IZyre`/`IDispatchMap`/`IMemoryTier`/
  `IRemoteLookupRdmaInitiator`/`IRemoteLookupRdmaResponder(+Admin)`), add an `rdma` feature that
  forwards to the initiator/responder crates' `rdma` features, and add `[dev-dependencies]` needed
  for the mesh harness. Keep the crate in `default-members`; no `spdk` dep.
- [X] T002 Create the module skeleton under `components/remote-lookup/src/`: `actor.rs`,
  `operation.rs`, `wire.rs`, `server.rs`, `seams.rs` (empty `//!`-documented stubs) and declare
  them as `mod` in `lib.rs`. Create `tests/mesh.rs` and `benches/correlation.rs` stubs.
- [X] T003 [P] Confirm `cargo fmt --check` and `cargo clippy -p remote-lookup -- -D warnings` pass
  on the skeleton (default rustfmt/clippy config; no new config files).

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Shared types and the actor scaffold that every user story builds on.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [X] T004 [P] Consume `LookupConfig` (defined in `interfaces` by T001a: `group`, `quorum_pct`,
  `phase1_timeout`, `op_deadline` default 50 ms — research Decision 9, `max_retry_rounds`,
  `max_keys_per_query`, `bind_ip`, `actor_cpu`, plus `discovery: Option<GossipConfig>` and
  `node_endpoint: Option<String>` for zyre peer-discovery (`None` ⇒ UDP beacon; `Some` ⇒ gossip, used
  by the in-process mesh and cross-subnet clusters — added as a follow-on `interfaces` commit), all
  `Default`). Confirm the field defaults match data-model.md and add any remote-lookup-internal
  derived config here (FR-022). (Depends on T001a.)
- [X] T005 [P] Implement the `wire` module in `src/wire.rs`: hand-rolled little-endian encode/decode
  for the v1 framed messages (header `[version=1][msg_type][op_id:u64]` + payloads for KeyQuery,
  KeyResponse, RdmaRequest, RdmaStatus) per data-model.md/`contracts/wire-protocol.md`. Unknown
  `msg_type` decodes to an `Unknown` variant that callers log-and-ignore (FR-018). Include unit
  tests for round-trip encode/decode and endpoint framing.
- [X] T006 [P] Implement mock seams in `src/seams.rs` and the `TestMesh` fixture in
  `tests/mesh.rs` (research Decision 8). **Zyre is NOT mocked** — each node uses the real `zyre`
  component. Mock only: (1) `IRemoteLookupRdmaInitiator`/`IRemoteLookupRdmaResponder(+Admin)` —
  channel-backed seams emulating `push`→per-key `PushStatus`, `local_region`/`local_endpoint`, and
  `Disconnect`→`DisconnectAck`, with the mock "push" moving no bytes (protocol-only); (2) scriptable
  `IMemoryTier`/`IDispatchMap`/`IDispatcher` — a per-node holdings table (memory/disk/absent per key
  at a given size) supporting scripted transitions (evict a key between its KEY_RESPONSE and the
  later RDMA_REQUEST; `promote_to_memory_tier` flipping BlockDevice→MemoryTier, with a failure
  variant). `TestMesh` spawns N instances, wires each to real zyre via gossip loopback (one hub
  `bind tcp://127.0.0.1:<free-port>`, rest `connect`; each node a distinct mailbox `endpoint`),
  provides a **discovery barrier** (all nodes see each other before the first SHOUT) and per-node
  **reply-delay**/**mid-protocol-exit** scripting. Assertions target timing-robust invariants only.
- [X] T007 Update the `define_component!` block in `src/lib.rs`: declare receptacles `zyre`,
  `dispatch_map`, `memory_tier`, `dispatcher` (US4 disk promotion — FR-016/FR-023), `initiator`,
  `responder`, `responder_admin`, `logger`; add fields `submit_tx: OnceLock<Sender<ActorMsg>>`,
  `actor: Mutex<Option<ActorHandle>>`, `op_counter: AtomicU64`, `config: LookupConfig`. (Depends on
  T004.)
- [X] T008 Define `ActorMsg` (`Submit(OperationRequest)`, `Join(String)`, `Leave`, `Shutdown`) and
  `OperationRequest { op_id, entries, done }` in `src/actor.rs`; wire the MPSC submission channel +
  per-op one-shot completion using `component_core::channel` (research Decision 2). (Depends on T007.)
- [X] T009 Implement the actor thread spawn/lifecycle + poll-loop scaffold in `src/actor.rs`
  (research Decision 1): each iteration drains submissions, `zyre.try_recv()`, responder
  `event_rx.try_recv()`, advances per-op deadlines, then bounded-sleeps (~500 µs–1 ms) only when
  idle; optional NUMA pin via `actor_cpu`. Wire `join_cluster`/`leave_cluster`/deactivate to
  `Join`/`Leave`/`Shutdown`. Route inbound wire messages by `msg_type`; discard replies whose
  `op_id` is not in the active-operation map (FR-019). (Depends on T008, T005.)

- [X] T009b Initialize the RDMA responder at startup (FR-025): via `responder_admin`, set the bind
  IP (from `config.bind_ip`) + `actor_cpu`, call `initialize()`, then read and cache the bound
  `Endpoint` (to advertise in KEY_RESPONSE) and the pool-wide `rkey` from `responder.local_region()`
  (consumed by T015's `RemoteRegion`), and open the responder control channel (`command_tx`/
  `event_rx`) polled by the loop (T009). Deregister/shutdown on deactivate after QPs are down.
  (Depends on T004, T007, T009.)

**Checkpoint**: Foundation ready — actor runs, responder initialized (endpoint+rkey cached), codec
+ seams exist, component wired.

---

## Phase 3: User Story 1 - Remote Memory-Tier Hit (Priority: P1) 🎯 MVP (client)

**Goal**: A local miss batch is fetched from a peer's memory tier over RDMA and published locally.

**Independent Test**: In the `TestMesh`, a requester node shouts a KEY_QUERY for `(K,S)` it lacks; a
peer whose scripted holdings report `(K,S)` in memory receives it; assert the requester whispers an
RDMA_REQUEST to that peer, reserves a private slot, and on RDMA_STATUS(Success) publishes the key and
`batch_lookup` returns `Ok(())` positionally.

### Tests for User Story 1 ⚠️ (write first, ensure they fail)

- [X] T010 [P] [US1] Multi-node mesh test in `tests/mesh.rs`: US1 happy path
  (KEY_QUERY→KEY_RESPONSE→RDMA_REQUEST→RDMA_STATUS(Success)→publish→`Ok(())`), acceptance
  scenario 1. **Canonical 4-node scenario** (also seeds T018/T020/T031): node1 shouts `[k1,k2,k3]`
  (holds none); node2 replies immediately `k1,k2 on disk, k3 in memory` but has k3 evicted before
  its RDMA_REQUEST (→ KeyNoLongerAvailable); node3 replies after a short delay `all three on disk`;
  node4 replies after a longer delay (still < `op_deadline`) `k1,k3 in memory only`. Assert the
  timing-robust invariants: k3 is requested from node2 first and fails, then re-targets to node4
  (the only other memory holder) and succeeds; k1 is satisfied from a memory holder (node4); k2 is
  satisfied from a disk holder; every key ends `Satisfied`; and no RDMA_REQUEST goes to a peer that
  did not advertise that key at size S. All four nodes concurrently run their own `batch_lookup`
  while serving, exercising the dual client/server role (SC-008).
- [X] T011 [P] [US1] Single-flight test in `tests/mesh.rs`: two concurrent
  `batch_lookup`s for the same missing `(K,S)` issue exactly one RDMA (follower blocks on the
  in-flight fetch), SC-008 / acceptance scenario 2.

### Implementation for User Story 1

- [X] T012 [US1] Implement the `Operation` state machine in `src/operation.rs`: fields per
  data-model.md (`op_id`, `keys`, `status`, `replies`, `phase`, `retry_round`, `peers_expected/
  replied`, `deadline`, `done`); per-key transitions Unsatisfied→InProgress→Satisfied; a
  `LandingSlot { key, addr, len, peer }` reserved via `memory_tier.insert` (publish-on-success —
  NOT published to dispatch-map yet). (Depends on T009.)
- [X] T013 [US1] Implement `initialize(LookupConfig)` and `batch_lookup` in `src/lib.rs`.
  `initialize`: store config, spawn the actor, drive responder bring-up (via T009b), join the group;
  idempotent (already-initialized error on repeat). `batch_lookup`: allocate `op_id` from
  `op_counter`, package `OperationRequest`, send on `submit_tx`, block on the one-shot until
  finalized; empty entry list returns an empty vec immediately. Remove the placeholder `NotFound`
  body. (Depends on T008, T012, T009b.)
- [X] T014 [US1] On `Submit`, SHOUT a KEY_QUERY for all `(key,size)` entries, splitting into
  multiple messages under one `op_id` when the batch exceeds `max_keys_per_query`; initialize
  per-op state + `deadline = now + op_deadline` (FR-005). (Depends on T012, T005.)
- [X] T015 [US1] On KEY_RESPONSE, greedily mark this peer's unsatisfied+not-in-progress **memory**
  hits in-progress and whisper an RDMA_REQUEST to that peer, reserving a private landing slot per
  key (`memory_tier.insert`) and building `RemoteRegion { addr, rkey: <cached pool rkey>, length }`
  (FR-006, FR-006a, FR-007); cache the peer's full reply for later phases. (Depends on T012, T014,
  and T009b for the cached rkey.)
- [X] T016 [US1] On RDMA_STATUS(Success) for a key, publish the landing slot
  (`create_memory_tier_entry(key, addr, len)` → `release_write(key)`), mark the key Satisfied
  (FR-008). On `AlreadyExists`, **check `entry_size(key)`**: equal ⇒ genuine success (someone else
  published the same key); different ⇒ size collision ⇒ do **not** report success — treat the key as
  unsatisfied, reclaim our private landing slot, and do not evict the existing entry (first-writer-
  wins; see `knowledge/size-mismatch-handling.md`). On failure statuses, return the key to
  Unsatisfied without reclaiming the slot yet (FR-009). (Depends on T012, T015.)
- [X] T017 [US1] Implement actor-side single-flight in `src/operation.rs`/`src/actor.rs`: a per-key
  in-flight index `HashMap<CacheKey, InFlight { serving_op, followers: Vec<op_id> }>`; a second
  same-key request attaches as a follower (no duplicate query/RDMA) and is completed when the fill
  resolves; a local reader that bounced back through the dispatcher is deduped as a follower
  (FR-026, SC-008). (Depends on T012, T016.)

**Checkpoint**: Client fetch path works end-to-end against a mock serving peer.

---

## Phase 4: User Story 2 - Server Role: Answering Queries (Priority: P1) 🎯 MVP (server)

**Goal**: Answer a peer's KEY_QUERY with per-key memory/disk/none classification.

**Independent Test**: Deliver a KEY_QUERY for a key held in memory / on disk / at a wrong size and
assert the whispered KEY_RESPONSE classifies each correctly (acceptance scenarios 1–3).

### Tests for User Story 2 ⚠️

- [X] T018 [P] [US2] Multi-node mesh test in `tests/mesh.rs`: KEY_QUERY classification
  for memory-hit, disk-hit, and size-mismatch→not-available (US2 scenarios 1–3).

### Implementation for User Story 2

- [X] T019 [US2] Implement KEY_QUERY handling in `src/server.rs`: for each `(key,size)` consult
  `dispatch_map` — memory-tier match with equal size ⇒ memory, block/disk match with equal size ⇒
  disk, else not available; whisper a KEY_RESPONSE echoing `op_id` with the requester endpoint +
  per-key classification (FR-015). Ignore any SHOUT whose peer id equals the local zyre uuid
  (FR-021). (Depends on T009, T005.)

**Checkpoint**: A node answers queries; combined with US1 two nodes complete the query half.

---

## Phase 5: User Story 3 - Server Role: Serving an RDMA Request (Priority: P1) 🎯 MVP (data path)

**Goal**: Serve a peer's RDMA_REQUEST by pushing the value via the initiator and reporting status.

**Independent Test**: Deliver an RDMA_REQUEST for a resident key and assert `push` is invoked and
RDMA_STATUS(Success) whispered; for an evicted key assert KeyNoLongerAvailable; for an unreachable
endpoint assert UnableToConnect (acceptance scenarios 1–3).

### Tests for User Story 3 ⚠️

- [X] T020 [P] [US3] Multi-node mesh test in `tests/mesh.rs`: RDMA_REQUEST success,
  evicted-key→KeyNoLongerAvailable, unreachable→UnableToConnect (US3 scenarios 1–3).

### Implementation for User Story 3

- [X] T021 [US3] Implement RDMA_REQUEST handling in `src/server.rs`: pin each requested value
  (`dispatch_map` read reference), delegate the write to `initiator.push(endpoint,
  [(key, RemoteRegion)])`, release pins, and whisper per-key RDMA_STATUS mapped from `PushStatus`
  (`Success`→Success, `UnableToConnect`→UnableToConnect, `KeyNotFound`→KeyNoLongerAvailable,
  `SizeMismatch`→KeyNoLongerAvailable) (FR-016). Disk→memory promotion is deferred to US4 —
  a disk-only entry here reports KeyNoLongerAvailable for now. (Depends on T009.)

**Checkpoint**: Two mock instances complete the full memory-hit round trip.

---

## Phase 6: User Story 6 - Completion Criteria and Timeout (Priority: P2) 🎯 MVP

**Goal**: Every operation finalizes deterministically; no hangs.

**Independent Test**: Deadline expiry with no replies → all `Err(NotFound)`; all-satisfied →
immediate completion; 80% quorum in a 10-node group fires the phase transition without the last two.

### Tests for User Story 6 ⚠️

- [X] T022 [P] [US6] Multi-node mesh tests in `tests/mesh.rs`: deadline→NotFound
  (scenario 3, SC-002), immediate completion when all satisfied (scenario 2), and the quorum/
  timeout Phase-1 transition trigger (scenario 1).

### Implementation for User Story 6

- [X] T023 [US6] Implement finalization in `src/operation.rs` (FR-012): finalize at the first of —
  all Satisfied; no cached peer holds any remaining key and no more replies expected; retry-round
  cap reached; or `deadline` elapsed. On finalize, discard unpublished landing slots
  (`memory_tier.remove`), send `done` with the positional `Vec<Result<(),_>>`, drop the `Operation`
  and clear its in-flight-index entries (waking followers). (Depends on T017.)
- [X] T024 [US6] Implement the Phase-1 transition trigger + deadline advancement in the poll loop
  (`src/actor.rs`): track `peers_expected` (group-size snapshot at SHOUT) and `peers_replied`; fire
  the transition at `quorum_pct` or `phase1_timeout`; enforce `op_deadline` per op (FR-006a timing,
  US6). (Phase-2 disk re-scan itself is deferred with US4.) (Depends on T023.)

**Checkpoint**: Operations always terminate; MVP timing behavior verified.

---

## Phase 7: User Story 7 - Peer Departure / Teardown-before-reclaim (Priority: P3) 🎯 MVP

**Goal**: A departing peer never hangs an operation and never causes a late write into a reclaimed
slot.

**Independent Test**: Inject a zyre `Exit` for an expected/in-flight peer; assert its cached reply
is dropped, its in-progress keys return to unsatisfied, completion re-evaluates, and any exposed
landing slot is reclaimed only after `DisconnectAck`.

### Tests for User Story 7 ⚠️

- [ ] T025 [P] [US7] Multi-node mesh tests in `tests/mesh.rs`: Exit drops cached reply +
  returns in-progress keys + re-evaluates (scenario 1); in-flight slot reclaim waits for
  `DisconnectAck` (scenario 2, SC-005/SC-006).

### Implementation for User Story 7

- [X] T026 [US7] Handle zyre `Exit` in `src/actor.rs` (FR-013): delete the peer's `PeerReply`,
  return its in-progress keys to Unsatisfied, and re-evaluate completion. (Depends on T023.)
- [X] T027 [US7] Implement teardown-before-reclaim in `src/actor.rs`/`src/operation.rs` (FR-014,
  SC-005): before returning a landing slot exposed to a departed/severed peer to the allocator,
  send `ResponderCommand::Disconnect { node }` and block for `ResponderEvent::DisconnectAck`;
  completing same-key waiters with not-found may happen immediately, only physical reclaim waits on
  the ack. (Depends on T026.)

**Checkpoint**: MVP complete — US1+US2+US3+US6+US7 pass on mock seams.

---

## Phase 8: User Story 4 - Disk Fallback (Priority: P2)

**Goal**: Satisfy a still-unsatisfied key from a peer's disk tier (serving node promotes disk→memory
before pushing). D3 resolved via the existing `IDispatcher::promote_to_memory_tier` — no interface
or dispatcher code change.

### Tests for User Story 4 ⚠️

- [X] T030 [P] [US4] Multi-node mesh tests in `tests/mesh.rs`: disk-only hit
  promoted+served; Phase-2 skipped when Phase-1 already satisfied all keys (US4 scenarios 1–2). The
  mock dispatcher seam models `promote_to_memory_tier` flipping a `BlockDevice` entry to
  `MemoryTier` (and a failure case that leaves it non-resident).

### Implementation for User Story 4

- [X] T028 [US4] Implement the Phase-2 disk re-scan in `src/operation.rs` (FR-010): after the
  Phase-1 transition, re-scan cached replies (no new SHOUT) for still-unsatisfied keys and whisper
  RDMA_REQUESTs to peers that reported the key on **disk**, preferring peers not already tried.
  (Depends on T024.)
- [X] T029 [US4] Implement server-side disk promotion in `src/server.rs` (FR-016/FR-017): for
  RDMA_REQUEST keys that `dm.lookup` reports as `BlockDevice`, call
  `dispatcher.promote_to_memory_tier(&[keys])` (batched), re-`dm.lookup` each; those now
  `MemoryTier{ptr,size}` proceed to pin+push, those still non-resident get
  RDMA_STATUS(KeyNoLongerAvailable). Requires the `dispatcher` receptacle (T007). (Depends on T021.)
- [X] T029a [US4] Document the teardown contract: the integrating mainline must
  `Receptacle::disconnect()` one side of the `dispatcher-p2p ⇄ remote-lookup` Arc cycle at
  deactivate. Documented in `quickstart.md` wiring (teardown note) and in the `full-remote.yaml`
  wiring comment. A dedicated lifecycle assertion is deferred: the mesh harness mocks the dispatcher
  (no bidirectional cycle to break), and a graceful disconnect in `certus-server-yaml` would need the
  generated composition to expose the component handle — the profile relies on process-exit teardown
  (benign leak) until that codegen enhancement lands.

---

## Phase 9: User Story 5 - Retry to an Alternate Peer (Priority: P2)

**Goal**: On a non-success RDMA_STATUS, re-target the key to an alternate cached peer within a
bounded number of rounds. Self-contained in the actor — no cross-component dependency.

### Tests for User Story 5 ⚠️

- [X] T032 [P] [US5] Multi-node mesh tests in `tests/mesh.rs`: retry to alternate peer on
  failure (scenario 1), retry-cap→NotFound (scenario 3), SC-004.

### Implementation for User Story 5

- [X] T031 [US5] Implement bounded retry rounds in `src/operation.rs` (FR-011): on a non-success
  RDMA_STATUS, return the key to Unsatisfied and re-target it to an alternate cached peer (memory
  preferred, then disk), never re-targeting a peer already known to have failed it in this
  operation; stop at `max_retry_rounds`. (Depends on T023; integrates with US4's disk preference.)

---

## Phase 10: Polish & Cross-Cutting Concerns

- [X] T033 [P] Add the SC-003 greedy-dispatch/correlation micro-benchmark in
  `benches/correlation.rs` (structural: RDMA_REQUEST whispered on the same event as the first
  satisfiable KEY_RESPONSE, no intervening poll cycle).
- [X] T034 [P] Hardware mesh loopback test (`#[ignore]`, `rdma` feature) in `tests/mesh_rdma.rs`
  (kept separate from the mock-seam `tests/mesh.rs`): two `remote-lookup` nodes over real localhost
  zyre with the mock RDMA seams swapped for the real initiator/responder crates on single-host RDMA
  loopback (distinct ephemeral ports), per quickstart.md. Validated on mlx5_0 (RoCE 10.0.0.102): the
  full fill path SHOUT → KEY_RESPONSE → RDMA_REQUEST → one-sided write → RDMA_STATUS(Success)
  completes and publishes locally. Fixed two harness bugs surfaced only on the real path: the gossip
  hub/mailbox must bind concrete ports (a literal `:0` left the connecting node nothing to dial), and
  `MockMemoryTier::peek`/`get` must expose `with_memory`-staged (server-side, resident-in `entries`)
  source pointers — the real initiator RDMA-reads its source through `IMemoryTier::get`, which the
  mock initiator never exercised.
- [X] T034a **[certus-server-yaml — app-level]** Integrate into the node stack: add an
  `init_remote_lookup` hook in `apps/certus-server-yaml/src/hooks.rs` that builds
  `LookupConfig { ..Default::default() }` (sourcing `bind_ip`/`actor_cpu` from `StackConfig` where
  available) and calls `initialize`; wire `remote_lookup`'s `init_hook`/`init_order`, the
  `dispatcher` receptacle, and the teardown `Receptacle::disconnect()` in
  `profiles/full-remote.yaml`. Confirm the profile composes (`cargo build -p certus-server-yaml`).
- [X] T035 [P] Update `components/remote-lookup/README.md` and rustdoc so `cargo doc -p
  remote-lookup --no-deps` is warning-free with runnable examples; run the
  `components/remote-lookup:component-update-docs` skill.
- [X] T036 [P] Sync the knowledge-base component description via
  `components/remote-lookup:wiki-update-component-design-descriptions`.
- [X] T037 Final gate: `cargo build`, `cargo test -p remote-lookup` (18 lib + 12 mesh + 2 doc),
  `cargo clippy -p remote-lookup --all-targets -- -D warnings`, `cargo fmt --check` — all green;
  ran `component-sync-specs` (2026-07-15) confirming code↔spec alignment: added FR-027 (warm-at-
  discovery) + FR-028 (off-loop worker) + the InitiatorWorker entity + T038/T039 for this session's
  connection-hardening work.
- [X] T038 [connection-hardening] Off-loop initiator worker (FR-028) in `src/worker.rs`: a dedicated
  thread runs the two blocking initiator ops (warm connect, serve one-sided write) off the poll
  loop; the poll loop hands work over a command channel and whispers each serve's RDMA_STATUS when
  the worker posts results back (`ActorMsg::PushComplete`). Keeps zyre event/status/teardown
  processing responsive during a multi-second cold `rdma_cm` connect.
- [X] T039 [connection-hardening] Warm-at-discovery (FR-027): advertise the responder endpoint in
  the zyre presence header; on `Enter` for a peer that advertises one, dispatch an off-loop
  `IRemoteLookupRdmaInitiator::connect` warm (best-effort). Test `warms_connections_to_discovered_peers`
  in `tests/mesh.rs`. Depends on the initiator's warm-connect method (initiator spec 002 FR-014).
- [ ] T040 [perf/gated] **Single-MR: share the responder's pool registration with the initiator.**
  Today the responder registers the whole memory-tier pool once at `initialize` (inbound
  `REMOTE_WRITE`) *and* the initiator re-registers the same pool per connection (source reads,
  `connection.rs` `register_existing_mr`). Have the initiator reuse the responder's already-registered
  region (its `lkey`, via `IRemoteLookupRdmaResponder::local_region` / a shared PD) instead of calling
  `ibv_reg_mr`, eliminating the per-connection registration.
  - **Gate — MEASURED (2026-07-15, mlx5), both page sizes:** `ibv_reg_mr` is linear in pool size —
    ≈40 µs/MiB (≈38 ms/GiB) on 4 KiB pages, ≈3.4 µs/MiB (≈3.5 ms/GiB) on 1 GiB hugepages (1 GiB
    3.5ms / 2 GiB 7ms / 4 GiB 14ms). Hugepages cut it ~10× but do NOT make it free (cost is kernel
    pinning + NIC translation-table population, both size-linear — not CPU-PTE-count bound). Harness
    `components/remote-lookup-rdma-initiator/tests/mr_registration_bench.rs` (`CERTUS_MR_BENCH_HUGE=1`),
    analysis in that crate's `info/DESIGN.md` ("Measured registration cost").
  - **Verdict: RECOMMEND DEFER for typical (≤ few-GiB) hugepage pools** — ~3.5 ms/GiB per peer,
    hidden by warm-at-discovery, is not worth the coupling: a raw `ibv_pd` handle crossing the
    component boundary + coupled MR/PD teardown lifetimes + NIC-per-NUMA per-device keying (see
    initiator DESIGN "Shared tier registration constraints"). **Worth doing only for very large
    per-node pools** (tens of GiB → hundreds of ms/connection × mesh fan-out × reconnects). Revisit
    if a deployment uses large pools. Cross-component (initiator + remote-lookup wiring); requires
    shared PD ⇒ shared device context (trivial single-NIC).

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: no dependencies. **T001a (interfaces change) is the first prereq** — it lands
  as its own `interfaces` commit and blocks T004/T007/T013.
- **Foundational (Phase 2)**: depends on Setup — BLOCKS all user stories.
- **US1 (Phase 3)**: after Foundational. The client half.
- **US2 (Phase 4)** and **US3 (Phase 5)**: after Foundational; independent of US1 (server halves) —
  can proceed in parallel with US1, but the meaningful mesh tests need US1+US2+US3 together.
- **US6 (Phase 6)**: depends on the Operation state machine (T012, T017).
- **US7 (Phase 7)**: depends on finalization (T023).
- **US4 (Phase 8)**: after the core MVP; depends on the Phase-1 transition (T024), the server push
  path (T021), and the `dispatcher` receptacle (T007). D3 resolved — no cross-component work.
- **US5 (Phase 9)**: after finalization (T023); self-contained. Recommended to land **before** US4
  (simpler, no new receptacle).
- **Polish (Phase 10)**: after the desired stories.

### Within Each User Story

- Tests written first and failing, then implementation.
- `operation.rs` state machine (T012) precedes client behaviors; `wire.rs` (T005) + `seams.rs`
  (T006) precede everything that sends/receives or is tested.

### Parallel Opportunities

- T004, T005, T006 (config/wire/seams) are independent — run in parallel.
- All `[P]` test tasks within a story run in parallel.
- US2 and US3 server halves can be built in parallel with US1 once Foundational is done.

---

## Parallel Example: Foundational

```bash
Task: "Implement LookupConfig in src/lib.rs"          # T004
Task: "Implement wire codec in src/wire.rs"           # T005
Task: "Implement mock seams in src/seams.rs"          # T006
```

---

## Implementation Strategy

### Increment 1 — core (memory-tier hits): US1 + US2 + US3 + US6 + US7

1. Phase 1 Setup → Phase 2 Foundational.
2. Build US1 (client), US2 + US3 (server) — the symmetric protocol needs all three to demo.
3. Add US6 (completion/timeout) and US7 (peer-exit teardown) for correctness/safety.
4. **STOP and VALIDATE**: `tests/mesh.rs` green (real zyre + mock NIC); then run the `#[ignore]`
   hardware loopback (T034) on the mlx5 box.

### Increment 2 — resilience & coverage: US5 then US4

- **US5 (retry)** first — self-contained in the actor, no new receptacle; adds transient-failure
  resilience.
- **US4 (disk fallback)** next — wire the `dispatcher` receptacle, call the existing
  `IDispatcher::promote_to_memory_tier`, and honor the teardown-disconnect contract (T029a). Adds
  hit-rate for cold data without changing the client contract.

Both are independently testable and additive — the core MVP contract is unchanged.

### Notes

- Publish-on-success (research Decision 5): dispatch-map is touched only on RDMA success; the
  failure/peer-exit path uses `memory_tier.remove` only — no dispatch-map change (D1 dropped).
- `op_deadline` is configurable (default 50 ms); derive the production value from measurement later
  (research Decision 9) — not on the MVP critical path.
- D3 (disk promotion) needs no cross-component code: `IDispatcher::promote_to_memory_tier` already
  exists. The only added coupling is the `dispatcher` receptacle + the teardown-disconnect contract
  (T029a) for the `dispatcher-p2p ⇄ remote-lookup` Arc cycle (research Decision 7).
- Keep the crate in `default-members`; the `rdma` feature only forwards to initiator/responder.
