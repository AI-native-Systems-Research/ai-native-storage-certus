---
description: "Task list for the certus-transfer-endpoint symmetric RDMA cache transfer"
---

# Tasks: Explicit certus-to-certus KV cache transfer over a symmetric RDMA endpoint

**Input**: Design documents from
`components/certus-transfer-endpoint/specs/001-rdma-cache-transfer/`

**Prerequisites**: plan.md, spec.md, research.md (D1–D15), contracts/ (no data-model.md,
no quickstart.md — `setup-tasks.sh` confirms both absent)

**Tests**: INCLUDED and first-class. Constitution Principles II–IV make unit tests, doc
tests and Criterion benchmarks mandatory, and six of the eight success criteria (SC-001,
SC-002, SC-004, SC-005, SC-006, SC-007) are assertions that only a test can discharge.

**Organization**: Grouped by **plan.md's implementation phases**, not by spec priority
order. This is deliberate and is the plan's own instruction: plan.md Phase 1 "satisfies P2
first, deliberately", so that the engine is proven before `remote-lookup` is touched.
Every task still carries a story tag for traceability, and the phase gates are the
success criteria named in plan.md.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel — different file, no dependency on an incomplete task
- **[Story]**: `[US1]` destination fetch (P1) · `[US2]` legacy push path (P2) ·
  `[US3]` block-tier serving (P3) · `[XC]` cross-cutting (serves more than one story)
- Paths are relative to `components/certus-transfer-endpoint/` unless noted
- Each task names the FR / SC / D / Principle it discharges; see the traceability matrix

## Path Conventions

Single Rust component crate per plan.md's Source Code layout: `src/` (lib, ffi, rdma,
connection, message, serve, bounce, telemetry), `benches/`, `build.rs`, `wrapper.c`.
Unit tests live in-module (`#[cfg(test)] mod tests`); the hardware loopback test is its
own `#[ignore]`d module, mirroring
`remote-lookup-rdma-initiator/src/loopback_test.rs`. The interface trait lives in
`components/interfaces/src/` (Principle I).

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: crate scaffolding, the feature matrix of D14, and the one upstream unblock
that Principle VIII requires.

- [X] T001 Create `Cargo.toml`: `[features] default = []`, `rdma = []`, `spdk = []`,
  `telemetry = []` (D14; `rdma` and `spdk` independently selectable per the
  constitution's Platform constraints); deps `component-framework`, `component-core`,
  `component-macros`, `interfaces`, `anyhow`; `[build-dependencies] cc`, `pkg-config`;
  `[dev-dependencies] criterion` with `html_reports`; `[[bench]]` stanzas with
  `harness = false`
- [X] T002 **[BLOCKER — cleared]** Ungate `IDispatchMap` and `LookupResult`
  in `components/interfaces/src/lib.rs` (drop the two `#[cfg(feature = "spdk")]`
  attributes on their re-exports) and confirm `idispatch_map.rs` compiles without the
  `spdk` feature. The module references **no** SPDK type — the only `spdk` tokens in it
  are the gate attributes themselves — so this is the "ungate those opaque-pointer trait
  definitions (they reference no SPDK type) as was done for `IMemoryTier`" change the
  workspace root `Cargo.toml` already names as the intended direction
- [X] T003 [P] Add `components/certus-transfer-endpoint` to workspace `members` **and**
  `default-members` in the root `Cargo.toml` (Principle VIII), with a comment explaining
  why it differs from `remote-lookup-rdma-initiator`/`-responder`, which are members but
  not default members because they link rdma-core unconditionally. Depends on T002:
  without it, the `IDispatchMap` receptacle of D13 forces `interfaces/spdk`, which pulls
  `spdk-sys` and makes default membership impossible — the exact trap documented for
  `remote-lookup`
- [X] T004 [P] Declare the module skeleton in `src/lib.rs` per plan.md's layout:
  `mod connection; mod message; mod serve; mod telemetry;`, `#[cfg(feature = "rdma")] mod
  ffi; #[cfg(feature = "rdma")] mod rdma;`, `mod bounce;` (block-tier reads, `spdk`), and
  `#[cfg(test)] mod loopback_test;` gated for Phase 9
- [X] T005 [P] Write the component `CLAUDE.md` (plan.md Constitution Check requires one):
  role, the symmetric-endpoint invariant, the feature matrix, and a pointer to Principles
  IX/X/XI as the review gates

**Checkpoint**: `cargo build` (default members, no rdma-core, no SPDK) compiles the crate
with empty module stubs, and `cargo build -p interfaces` still succeeds both with and
without `--features spdk`.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: the interface, the single FFI copy, the transport seam, and the symmetric
connection table. This is plan.md Phase 1's engine.

**⚠️ CRITICAL**: no story work can begin until this phase is complete.

- [X] T006 Define `ITransferEndpoint` in
  `components/interfaces/src/itransfer_endpoint.rs` via `define_interface!`, **ungated**
  (Principle I, contracts/itransfer_endpoint.md): methods
  `landing_region`, `fetch`, `push_async`, `push`, `disconnect`, `stats`, plus
  types `EntryOutcome`, `TransferContextId`, `LocalSlot`, `LandingRegion`,
  `TransferEndpointError`, `FetchCompletion`, `TransferStats`. Reuse the existing
  `RemoteRegion` and `CacheKey` verbatim. **No `serve()` method** — serving is reactive
  by design, and exposing it would invite callers to schedule it
- [X] T007 Write `src/ffi.rs` as **the single copy** of the verbs FFI (SC-008): the 52
  public names currently shared by
  `remote-lookup-rdma-initiator/src/ffi.rs` and `remote-lookup-rdma-responder/src/ffi.rs`,
  plus the send/recv surface neither crate ever used — `ibv_post_recv`, the recv work
  request and SGE types, and `IBV_WR_SEND` promoted from a bare unused constant to a
  live opcode (D3). Every `unsafe` item carries a `// SAFETY:` comment (Principle V).
  *Done:* structs were measured against the rdma-core 2510.0.11 headers, not copied —
  four predecessor definitions were wrong (`ibv_send_wr`'s `wr` union 24 B vs 32 B,
  `IBV_WR_*` numbering, `ibv_port_attr` missing `active_speed_ex`, guessed padding on
  `rdma_cm_id`/`rdma_cm_event`/`ibv_qp`). A `const` layout check pins every size and
  every Rust-touched offset; extern declarations carry a module-level `# Safety` caller
  contract and the `// SAFETY:` comments go at call sites in `rdma.rs`. epoll/eventfd
  are deliberately absent until T009/T010 choose the event loop
- [X] T008 Write `build.rs` + `src/wrapper.c` for the static-inline verbs shim, compiled
  by `cc` and located by `pkg-config`, **both gated on `rdma`** so the default build
  invokes neither (Principle VIII). Copy the initiator's shim as the model.
  *Done:* thin pass-through shims (`certus_te_post_send`/`_post_recv`/`_poll_cq`/
  `_query_port`/`_qp_to_error`), work requests built in Rust so chains and `bad_wr`
  survive; `wrapper.c` also carries the `_Static_assert` half of T007's layout check
- [X] T009 Write `src/rdma.rs` safe wrappers over T007: a `ProtectionDomain` owning one
  PD on one device, QP creation plus a device-reported state query and the explicit
  transition to ERROR (rdma_cm drives INIT → RTR → RTS itself, so there is no manual
  transition to wrap), `MemoryRegion` over caller-owned memory (the initiator's
  registration path already covers "memory owned by the caller (e.g. memory-tier pool)",
  D15), `post_send`, `post_recv`, completion polling. Gated on `rdma`. **The protection
  domain is owned by the endpoint for its lifetime, not by a connection** (D17) — unlike the
  initiator, where the PD lives in the connection (`rdma.rs:169`, `ibv_dealloc_pd` in its
  `Drop`) and the pool is re-registered on every connect (`connection.rs:1131`). Explicit QP
  transition to ERROR is part of this surface: it is what FR-023 relies on.
  *Done:* teardown order is enforced by `Arc` (region/CQ/QP each hold what they depend on),
  and a release that still fails panics (Principle XI). `post_send`/`post_recv` are
  `unsafe fn` whose only contract is buffer lifetime — bounds and `lkey` are checked by
  `MemoryRegion::local` — and post a chained batch, reporting `accepted` from `bad_wr`.
  Typed `RdmaError` carries the real errno. Also in scope: `EventChannel`, `CmId` (owns
  its QP), and a `CmEvent` guard that acks exactly once; CQ sharing is left to T010/T016.
  QP state is read with `ibv_query_qp`, not `ibv_qp::state` — the latter is libibverbs'
  cache of what software last set and misses a device-forced ERROR (the initiator's
  `is_qp_healthy` reads the cache). 4 `#[ignore]`d hardware tests pass on `mlx5_0`
- [X] T010 Define the **transport seam** in `src/connection.rs`: traits for a listener
  and a peer link that carry a QP and can *both* post and receive, with `CmEvent`-style
  inbound delivery. **The seam must be selected per build, with the interface working in
  every build** — do *not* copy the initiator's
  `#[cfg(not(feature = "rdma"))] push_async -> Err(NotInitialized)` pattern, which makes
  the interface untestable without hardware. D14 and contract invariant 2 both require
  the default build to *serve* over the mock seam, so the seam is the implementation of
  the interface, not a bypass of it (Principle VIII, Workflow rule 2 "Seam First").
  *Done* in `src/connection/transport.rs`, with a minimal mock in `src/connection/mock.rs`
  (`connection.rs` became `connection/mod.rs`). The seam is **verbs-shaped** — `wr_id`s,
  posted receives, per-link completions — so T013/T014/FR-026/FR-023 are testable without
  a NIC. `Transport` (listen, register, connect/accept/reject, events) and `Link` (post,
  poll, `to_error`, state, disconnect) are trait objects; connect is event-driven and the
  caller decides accept/reject (D23). **One endpoint thread owns every link; one CQ per
  link.** The mock is compiled in every build; `select_transport(BuildDefault)` is the
  process fabric without `rdma` and an explicit error with it until T053 (no silent
  fallback to the mock, Principle XI). `RdmaError`/`PostError`/`QpState`/`QpCaps`/`Access`
  moved out of `rdma.rs` into the seam; `rdma.rs` posts `RawSge`s. The minimal mock
  already models in-order execution with an infinite-retry RNR stall, checked one-sided
  writes (bad `rkey` → `REM_ACCESS_ERR`, both ends ERROR), flush on ERROR, capacity, and
  unsignaled slots retiring only behind a later completion
- [X] T011 [P] Extend the in-process mock in `src/connection/mock.rs` (depends T010) with
  fault injection: injectable inbound messages, a finite RNR retry, `LOCAL_WRITE` checks on
  receive buffers, and recorded QP→ERROR ordering so teardown-before-reclaim is assertable.
  Two endpoints in one process and the one-sided write that copies into the target address
  already exist (T010).
  *Done*. The recorded ordering is the event log (T021). Injected inbound messages are met
  at the endpoint (`Command::Inject`) and by raw transports in the tests, which every
  malformed-message test uses; a fabric-level injector would be a third way. **Finite RNR
  retry**: `rnr_retry_limit(Some(n))` fails a send `RNR_RETRY_EXC_ERR` after n stalls and
  forces both ends to ERROR. With it at zero, a deterministic burst — 8 replies and 8
  requests released at once onto one peer — completes with no stall when the receive
  partition is posted, and fails loudly at the ninth message when the reply reserve is
  withheld (a test-only `NoReplyReserve`): FR-026's deadlock made visible. **Receive
  checks**, set from probes on mlx5: a send longer than its receive fails the receive
  `LOC_LEN_ERR` and leaves both queue pairs up — the mock had been failing the send and
  forcing ERROR, so the endpoint's own teardown on a failed receive went untested, and now
  is; a receive into memory without `LOCAL_WRITE` lands on mlx5 but fails `LOC_PROT_ERR`
  here, which is verbs' rule, deliberately stricter. Both reference behaviours stay checked
  by an `#[ignore]`d hardware test. Six planted bugs, all caught. Timing is not modelled
- [X] T012 Implement the **symmetric** `ConnectionTable` in `src/connection/` (depends
  T010, T068, D5): one QP per peer **instance** pair keyed by `PeerId`, established on first
  need and reused. Remove the temporary per-item `#[allow(dead_code)]` attributes T068 left
  on `PeerId`/`PeerIdError` once this consumes them
  (FR-002), with a connection usable regardless of whether it was inbound-accepted or
  outbound-initiated, and **either end able to post work requests** (FR-001). This is the
  thing neither existing crate can do.
  *Done* in `src/connection/table.rs` + `src/connection/identity.rs`, and **absorbs T081**:
  a table cannot file an inbound link without the initiator's identity, and once it can, it
  meets duplicates, for which both stopgaps are wrong (reject-all livelocks a race,
  accept-all keeps two connections). Identity format, name-echo check and the two
  "newer wins" cases are research **D24**. Teardown is two-step — `begin_teardown` forces
  ERROR and moves the link to a draining set, `finish_teardown` destroys it after the
  caller's drain (T019/T013) — and a draining peer refuses reconnect at both ends (FR-023).
  No automatic retry; self-connect is an error. T068's `dead_code` allows removed
- [X] T013 [P] Implement the `wr_id` slab in `src/connection/` (depends T010): allocate
  a slot per posted work request carrying the per-entry state its completion must release
  (pin, bounce buffer, context + entry index). This is the mechanism Principle IX requires
  for "released by the completion reaper that owns its work-request id".
  *Done* in `src/connection/slab.rs`: generic `Slab<T>`, one per link, held by the table
  (`ConnectionTable<T>`; `live(peer)` was the only way to post — T014 replaced it with
  `post_sends`/`post_recvs`/`reap`). `wr_id` = slot index
  (low 32) | **generation** (high 32): draining links are polled after ERROR, so a reused
  slot can meet its previous occupant's late completion — the predecessor's slab was safe
  only because recovery destroyed the CQ. State goes in at `insert` and comes back only via
  `complete` or a partial post's `unposted` list. Tracked sends are always signaled.
  `post_{send,recv}_tracked` own the partial-failure hazard; `drain` polls an ERROR link
  until the slab is empty, failing loudly on a stall. `finish_teardown` refuses
  (`Undrained`) while a slab holds state, and dropping a table that holds state panics.
  The `entry_push` bench stays with T057
- [X] T014 Implement **send-queue credits** in `src/connection/` (depends T012, T013):
  bound in-flight RDMA writes per connection, with partial batch posting and a completion
  reaper that refills. First of the three domains of D10/FR-014; the other two arrive in
  Phase 7.
  *Done* in `src/connection/credits.rs` + the table: `SendCredits` per live link, limit
  = min(configured, granted `max_send_wr`), a clamp reported once as
  `TableEvent::CreditsClamped`; zero credits refused at construction. **Counts every
  send-queue request, not only writes** — requests and replies fill the same queue, so
  counting writes alone would let messages fill it under writes that think they have
  room (a reply holds a credit a write could use; accepted). Receives hold none (FR-026
  partitions them, T080). `post_sends(peer, &mut VecDeque)` posts from the front as far
  as credits go and leaves the rest; a transport refusal requeues the unposted at the
  front in order and returns their credits. `reap(peer)` is the only way to poll a live
  link and refills one credit per send-queue completion, so credits and slab cannot
  drift (`debug_assert`ed). `live()` is gone: a raw link would let a caller poll around
  the credits. Held pins ≤ in-flight ≤ limit by construction; T025 still owes the
  end-to-end SC-004 test. `peak` kept for T015
- [X] T015 [P] Implement `src/telemetry.rs`: ZST no-op with
  `#[allow(clippy::unused_self)]` when `telemetry` is off, `AtomicU64` counters when on,
  identical method surface. Counters: per-outcome totals, each credit domain's current and
  peak depth, chunk count, deadline expiries. Principle XI forbids structuring these so an
  unexercised path looks like a working one — so count *attempts* alongside completions.
  *Done, with the task text overridden:* `ITransferEndpoint::stats()` returns
  `TransferStats` in **every** build, so gating its fields would make the default build
  report zeros — the plausible-number-from-an-unexercised-path XI forbids. **Two tiers**:
  `Counters` (every `TransferStats` field) is always counted, as plain integers on the
  endpoint thread (atomics vs snapshot is T016's call); `Extras` (connect attempts /
  established / failed — not in `TransferStats`) is the feature-gated ZST, whose snapshot is
  `None`, not zeros, when off. Send-queue gauges are read from the live links' credits at
  snapshot time; the peak is folded into `Counters` when a link retires so it survives.
  Filled now: send-queue in-flight + peak, `duplicate_connections_resolved` (once per race,
  at the end whose attempt was discarded). Every other field names the task that fills it
- [X] T068 [P] Implement `PeerId` in `src/connection.rs`: parse and normalize a
  `"ip:port"` instance address, rejecting a missing port and port 0 rather than defaulting
  either (FR-021, D16). Reuse the grammar of
  `remote-lookup-rdma-initiator/src/connection.rs:1075` `parse_endpoint` rather than
  inventing a second one. Unit-test that two addresses differing only by port are distinct
  keys, that a host-only address is rejected, and that both may be the local host
- [X] T069 *(moved into T053)* Implement protection-domain ownership in `src/rdma.rs` (depends T009): T009's
  `ProtectionDomain` is one PD on one device; this task is the **per-device table**,
  lazy allocation after address resolution, and shutdown ordering — **one PD
  per device, owned by the endpoint for its lifetime**, with memory regions registered into
  it once and outliving every queue pair, so an `rkey` already on the wire stays valid
  across a reconnect (D17). This is a deliberate departure from the predecessor crates, and
  carries four consequences the decision accepts explicitly: (a) pass the PD to
  `rdma_create_qp` rather than accepting `id->pd`, allocating it lazily per `ibv_context`
  after address resolution and keying it per device, since a PD cannot be used with an id on
  a different device; (b) one shared PD means any connected peer can address any registered
  region — acceptable only under the spec's single-tenant trust assumption; (c) shutdown must
  tear down every QP and deregister every region before `ibv_dealloc_pd`, which fails
  `EBUSY` otherwise; (d) rewrite `MemoryRegion`'s `// SAFETY:` justification — the
  initiator's "only accessed from the thread that created it" (`rdma.rs:126`) stops being
  true once one region is shared across per-peer connection threads, and `Sync` may be
  needed alongside `Send` (Principle V). *(d) was discharged by T009:* `MemoryRegion` is
  `Send + Sync`, justified by its `ibv_mr` being immutable after registration. Key the
  table by the `ibv_context` pointer librdmacm hands the id (`ProtectionDomain::same_device`):
  `rdma_create_qp` rejects a PD from any other context, even one opened on the same device
- [X] T016 Wire `define_component!` in `src/lib.rs` (depends T006, T012): receptacles
  `logger: ILogger`, `dispatch_map: IDispatchMap` (both ungated, per D13 and T002), and a
  `dispatcher: IDispatcher` receptacle gated on `spdk` (T083). Endpoint owns a dedicated actor thread, as both
  predecessor crates do.
  *Done*: `CertusTransferEndpoint` provides `ITransferEndpoint` and a new
  **`ITransferEndpointAdmin`** (`configure(EndpointConfig)` / `initialize` / `shutdown`, in
  `interfaces`, ungated) — the interface carried no lifecycle. Until `initialize`, every
  transfer method returns `NotInitialized`. The thread is the framework `Actor`
  (`src/endpoint.rs`), with **panics made fatal**: every handler path runs under
  `fatal_on_panic`, which aborts rather than let the framework swallow a broken pin, slab or
  credit invariant. Interface methods are `Command`s; `stats`/`disconnect` wait on a one-shot
  reply. A peer whose link goes `Down` is torn down and drained on the thread at once.
  `shutdown` tears down and drains every peer before dropping the table; a stalled drain is
  reported and the table leaked rather than destroyed holding state. **Shutdown is the only
  way to stop it**: `define_component!` keeps `Arc`s of the component in its own interface
  map, so it is never dropped by releasing handles (pinned by a test). `fetch`, `push_async`,
  `push` validate their peer and return `NotInitialized("…lands
  in Txxx")`

**Checkpoint**: the engine builds and the mock seam carries a message and a simulated
write end to end, with neither rdma-core nor SPDK present.

---

## Phase 3: User Story 2 — the legacy push path keeps working (Priority: P2)

**Goal**: the messaging-free subset of FR-019, so `remote-lookup` could be re-pointed with
a receptacle change and no logic change — while leaving it untouched (FR-020).

**Independent Test**: the existing `remote-lookup` test layer, plus the initiator's
hardware loopback scenario, pass against this component with messaging unused.

**Gate**: **SC-002**.

- [X] T017 [US2] Implement `push_async(endpoint, &[(CacheKey, RemoteRegion)],
  on_complete)` in `src/serve.rs` + `src/lib.rs`: resolve or open the connection, post one
  RDMA write **per entry** (FR-008 — landing slots are not contiguous, so there is no
  batch write to post), accumulate outcomes, fire `on_complete` once with one outcome per
  item in request order. Post continuously: refill on each completion and never drain the
  send queue between entries while unposted entries remain (FR-022). A window of 2-4 already
  saturates the link at 2.25 MiB per entry, so this is about avoiding idle, not depth (D19).
  *Done* on the endpoint thread (`src/endpoint.rs`, `src/serve.rs`). Entries are looked up —
  and pinned — **lazily, only as send credits free**, so pins held never exceed the credits
  and nothing is pinned while a connection is still being made. A write failure tears the
  connection down; every exit (completion, failure, peer down, disconnect, shutdown) releases
  each pin exactly once and fires `on_complete` once. New receptacle `memory_tier`, whose
  pool is registered once at initialize (LOCAL|REMOTE_WRITE), the responder's approach.
  Block-tier answers release their pin and report `Miss` until T037-T040. **Adds
  `EntryOutcome::Failed`** (and `TransferStats::failed`, wire outcome byte 3): a fault is
  never reported as a miss
- [X] T018 [US2] Implement blocking `push` over `push_async` (contract: one outcome per
  item, in order). *Done*: waits on a one-shot channel the callback fills, which
  `push_async`'s exactly-once guarantee makes safe. The contract says `push` must not be
  called from a completion callback; called there it would wait on its own thread forever,
  so an endpoint thread is marked (thread-local, set in `on_start`) and `push` is refused
  there with an error naming `push_async`
- [X] T019 [US2] *(done in T016)* Implement `disconnect(peer)`: transition the peer's QP to ERROR, drain and
  discard its completions, and do not return until no write from that peer can still land
  (FR-017, Principle IX's last clause). This is the existing `Disconnect`/`DisconnectAck`
  barrier, kept — and it is also the mechanism the deadline path reuses (FR-023, T070), so
  it must abort every context outstanding on the connection, not just one
- [X] T020 [US2] Land in the memory-tier pool, registered once at `initialize`, not per
  request; an `rkey` is only valid because of it (wire-protocol.md "Connection and
  lifecycle"). Register into the long-lived PD of T069 so its `rkey` survives a reconnect
  (D17); depends on T069. `fetch` takes `LocalSlot { addr, length }` per key — a memory-tier
  buffer the caller has allocated and not published — refused unless inside the pool;
  `landing_region()` reports the pool's `addr`/`len`/`rkey` for a pusher. There is no
  separate landing-region registration: the only destination is the pool.
  *Done against the seam, without T069*: registration belongs to the endpoint, not to a
  connection, in every transport, and survives a reconnect (tested). T069's per-device PD
  table exists only to make the **verbs** transport keep that promise, so it moved into
  T053. No deregistration (proving no peer can still write is the FR-017 barrier
  generalised). A slot outside the pool, by a byte at either edge or by wrapping, is
  refused before anything is sent (tested)
- [X] T021 [US2] Tests in-module: order-and-length invariant 1 over the mock seam
  (`items.len()` outcomes, index *i* ↔ `items[i]`); connection reuse across repeated
  pushes (FR-002); teardown-before-reclaim ordering asserted from the mock's recorded
  QP→ERROR sequence (invariant 8); and a `#[test]` that the default build serves a
  memory-tier entry through the **interface**, not merely through `connection`.
  *Done*, building T011's ERROR-ordering piece for it: the mock fabric records an event log
  (ToError, each Completed handed out, Closed) stamped from a `Sequence` the ledger dispatch
  map also stamps its releases from, so the test asserts one order — ERROR, then the drain's
  completions, then each pin's release, then the link's destruction

**Checkpoint**: SC-002 — the legacy surface is complete and tested with zero messaging,
and `remote-lookup`'s logic has not been modified (its test doubles gained the T083 bounce
methods).

---

## Phase 4: Pin discipline (cross-cutting)

**Purpose**: plan.md Phase 2. Verifiable with no messaging at all, which is why it
precedes Phase 5. Principle IX is the review gate; changes here "MUST be reviewed against
Principle IX explicitly" (Workflow rule 6).

**Gate**: **SC-004**.

- [X] T022 [XC] Implement the per-entry serve step in `src/serve.rs`: **one**
  `dispatch_map.lookup(key)` yields residency *and* the pin *and* the address
  (`MemoryTier{pointer,size}`) or the extent offset (`BlockDevice{offset}`) together
  (FR-009, D6). Do not use `Check` to pick the tier first — it releases immediately and
  would reintroduce exactly the probe-then-pin window the dispatch map is built to avoid
- [X] T023 [XC] Release exactly what `lookup` pinned, exactly once (FR-010, D6 corollary,
  Principle IX). `NotExist` (and `MismatchSize`, should a map return it) are **unpinned**:
  releasing on them decrements another reader's pin — the live `Dispatcher::check()` defect
  (`exists = !matches!(result, NotExist)` then release), tracked separately and **not**
  reproduced here. But `lookup` takes no size and the current map never returns
  `MismatchSize`: a **wrong-size entry comes back `MemoryTier`, pinned**, and must be
  released before reporting `Miss`, or it is never evictable. The same for a block-tier
  answer this build cannot serve. Every pin is a `serve::Pin`, which panics if dropped
  unreleased
- [X] T024 [XC] Release a memory-tier pin from the completion reaper that owns its
  `wr_id`, per entry, never per batch (FR-011, D7, depends T013)
- [X] T025 [XC] Test **SC-004**: with send-queue credits set to *n*, a push of
  *n* × 100 entries never holds more than *n* concurrent pins. Instrument the mock
  dispatch map to record peak concurrent `read_ref`, and assert the bound independently of
  `items.len()` (invariant 5)
- [X] T026 [XC] Test the double-release hazard directly: a request mixing hits,
  `NotExist`, a wrong-size entry, a block-tier entry and `MismatchSize` leaves every key's
  `read_ref` at exactly its starting value — including another reader's pin left untouched. An over-release corrupts an unrelated reader and no ordinary test catches it —
  Principle IX's rationale asks for this one specifically

**Checkpoint**: SC-004 holds, and the pin ledger balances across mixed outcomes.

---

## Phase 5: User Story 1 — destination fetches a prefix from a named peer (Priority: P1) 🎯 MVP

**Goal**: plan.md Phase 3. The verbs request/reply layer that replaces zyre broadcast with
one explicit point-to-point ask.

**Independent Test**: two endpoints over the mock seam; requested keys land in the named
slots and the reply's per-key outcomes match the source's actual residency.

**Gate**: **SC-001** — zero zyre messages, zero messages to any third node.

- [X] T027 [US1] Implement the 24-byte header codec in `src/message.rs`: `opcode` naming a
  **kind only** (`OP_FETCH = 2`, `OP_FETCH_REPLY = 3`, `OP_HELLO = 0xFFFC`,
  `OP_HELLO_REPLY = 0xFFFD`, `OP_ERROR = 0xFFFF`; a reply is its request with the low bit
  set; high 16 bits MUST be zero), `flags` (reserved, MUST be zero),
  **`version` at offset 6**, `context_id`, `n_entries`, reserved fields that MUST be zero.
  Little-endian.
  Dispatch on `opcode` alone and **validate `version` once** against the connection's
  negotiated version — a consistency check, not a dispatch key, answered with `OP_ERROR`
  reason 2 on mismatch (D21). Do not reintroduce a fused opcode: a bump would mint a new
  opcode for every kind including unchanged ones, and the handler would mask the version
  straight back off. Bytes 0-15 (`opcode`, `flags`, `version`, `context_id`) are
  the **frozen prefix** — identical in every version of every kind, forever, because they are
  the minimum needed to reject a message, say *why*, and still correlate the rejection
  (D20, D21). Encode that as a type, not a convention: a receiver parses the prefix first,
  so whatever it cannot understand it can still name, refuse and correlate.
  *Done* (slice A, with T028, T029, T074 and the `OP_HELLO`/`OP_HELLO_REPLY` codecs) in
  `src/message.rs`: `Prefix` is the frozen 16 bytes, `Header::parse` checks the version
  once, every reserved field must be zero on receipt, and `refusal` builds the `OP_ERROR`
  for an unparseable request and refuses to answer a reply
- [X] T028 [P] [US1] Implement the 24-byte entry codec: `(key u64, addr u64, size u32,
  rkey u32)` — exactly `(CacheKey, RemoteRegion)`. **Absolute addresses, not slot
  indices**: `IMemoryTier` is an allocator keyed by `CacheKey` with per-entry size and has
  no index-addressable slot table, so indices would invent a layout the tier does not
  have. The per-entry `rkey` rides in what would otherwise be alignment padding and lets
  one request land across several registered regions
- [X] T029 [P] [US1] Implement the reply codec: one outcome byte per entry in request
  order (`0` Miss, `1` HitMemoryTier, `2` HitBlockTier, `3` Failed), zero-padded to an 8-byte
  boundary. A receiver MUST treat an unknown outcome (≥ 4) as `Miss`, never as an error,
  so a future source can add a tier without breaking an old requester
- [X] T074 [P] [US1] Implement the frozen `OP_ERROR` codec in `src/message.rs`
  (`0x0000_FFFF`, unversioned, D20): 24-byte header plus `offending_opcode: u32`,
  `reason: u16`, `max_supported_version: u16`. Reason codes 1 unknown opcode, 2 unsupported
  version of a known kind, 3 malformed message, 4 `n_entries` exceeds the posted buffer; 0
  reserved, >= 5 treated as a generic protocol failure and **never** as success. Its layout is
  frozen permanently — extensibility is through the reason space, not the layout — so add a
  test asserting the encoded size and field offsets, which is the thing a future change would
  silently break
- [X] T075 [US1] Answer every unprocessable inbound **request** with `OP_ERROR` rather than
  dropping it (FR-024), in place of its success reply: echo the `context_id`, name the
  offending opcode, and report the local highest supported version for that kind. Classify an
  unknown kind by the opcode's low bit: even is a request, answered; odd is a reply, a
  teardown. A request before `OP_HELLO`, and a duplicate `OP_HELLO`, are answered too, and the
  connection kept: teardown is only for FR-024's four cases. On **receipt** of `OP_ERROR`, fail that one context with
  `Protocol(..)` and leave the connection usable — the FR-023 teardown must not run and
  sibling contexts must be unaffected. **Never answer a reply.** A reply that cannot be
  processed (malformed, wrong kind, unknown context, duplicate) is logged loudly with the peer
  and the offending bytes, counted as a protocol fault, and tears the connection down through
  T070's ordering. Count both in telemetry — `protocol_faults` counts every `OP_ERROR` sent or
  received and every teardown an inbound message causes; an unexercised path must not look
  like a working one (Principle XI)
- [X] T076 [US1] Implement the frozen `OP_HELLO` codec and exchange (`0x0000_FFFC`,
  unversioned, FR-025, D20): a **fixed-size** 40-byte message — `max_entries: u32`,
  `max_outstanding_requests: u32`, `version_min: u16`, `version_max: u16`, reserved `u32`.
  `OP_HELLO` is a request: it costs the connection's one **bootstrap credit** and is answered
  by `OP_HELLO_REPLY` (`0x0000_FFFD`, unversioned, header only, **no content ever**) purely to
  balance credits — it must never carry capabilities or a verdict, or D20's second decision
  point returns. On the peer's `OP_HELLO`, set the message-credit limit to its advertised
  quota; a quota of 0 fails the connection. **Both** ends send it immediately on connection
  establishment, since the endpoint is symmetric and neither side is the client (D5); no data
  message goes out until the peer's has arrived. Store capabilities on the connection so they
  die with it and a rebuilt connection re-learns them. Fixed size is deliberate: the layout is
  frozen forever, so a variable-length body would be the wrong shape (D20 *Revision*)
- [X] T077 [US1] Apply the negotiated capabilities: the connection's version is
  `min(a.version_max, b.version_max)`, carried in the header's `version` field (offset 6) of
  every data-plane message thereafter and validated once on receipt (D21); chunk against the **peer's** `max_entries` rather than the local one — the
  two sides are not assumed to have equal receive buffers, which nothing checked before this
  exchange existed. Fail the **connection** with `IncompatiblePeer(..)` at establishment if the
  negotiated version falls below `max(a.version_min, b.version_min)`, naming peer and both
  ranges; never defer that to a request or a deadline. Count version downgrades
  (Principle XI). **One version governs the whole protocol** — do not add per-kind
  negotiation, which would permit a v2 request answered by a v1 reply (D20 *Revision*). The
  selection rule MUST be commutative, with no arbitrator: add a property test asserting that
  for every pair of ranges both argument orders give the same version and verdict (SC-014)
- [X] T078 [US1] Tests for T074-T077: **SC-012** — an unknown opcode injected over the mock
  seam draws an `OP_ERROR` within a round trip, the context fails with `Protocol(..)`, and the
  connection's sibling contexts still complete (assert the QP was *not* forced to ERROR, the
  contrast with T071); **SC-013** — a peer advertising a smaller `max_entries` is never sent a
  larger request, and a peer whose version range does not intersect the local one fails at
  connect with no request issued; a reason code >= 5 is treated as a failure, not success;
  `OP_ERROR`/`OP_HELLO`/`OP_HELLO_REPLY` round-trip at their frozen offsets and fixed sizes;
  a malformed reply is **not** answered and tears the connection down, while an `OP_ERROR`
  for our request does not; and the bootstrap credit allows exactly one message before the
  peer's `OP_HELLO`
- [X] T079 [US1] Make the connection's books per direction (FR-026, D22): **separate**
  outbound-request and inbound-serving context tables, since the requester allocates ids and
  one id can be live in both directions on a connection — a single contexts-per-connection map
  silently conflates two unrelated requests — plus per-direction message credits.
  *Credits per direction done in slice B* (`Session`: outbound limit and in-flight, inbound
  quota and in-service, counted separately); the two context tables are slice C.
  *Context tables done in slice C*: the outbound book is each connection's `outbound` map
  (our `context_id`s, numbered from 0 per connection), the inbound book the serving pushes
  (`Finish::Reply`, the peer's ids); `the_same_context_id_live_both_ways_is_two_requests`
- [X] T080 [US1] Partition posted receive buffers (FR-026, D22): reserve **exactly one buffer
  per own message credit** for replies — exact because each request message draws exactly one
  reply (FR-007, T045) — and advertise a separate inbound-request quota as the peer's
  `max_outstanding_requests`; post reserve + quota in all. Clamp this end's effective message
  credits to the quota the *peer* advertised, and enforce the granted quota on inbound
  requests. Without this, a peer issuing requests up to the buffer count takes every buffer
  and leaves none for a reply this end is blocked on: an RNR stall, and with two-way traffic
  a deadlock. Every message that can arrive is in a partition: `OP_HELLO` is a request
  (quota), `OP_HELLO_REPLY` and `OP_ERROR` are replies (reserve), and replies are never
  answered, so nothing lands outside either. Post one reserve and one quota buffer before the
  connection is established, for the bootstrap exchange
- [X] T081 *(absorbed into T012 — see there and D24)* Resolve simultaneous connect (FR-027, D23): on detecting a duplicate, both ends keep
  the connection whose **initiator has the lower `PeerId`** and close the other — a
  deterministic function of two known values, no arbitrator, no extra round trip. Requires the
  initiator to identify itself in the connect `private_data` (D16 puts a certus instance id
  there, replacing the predecessor's zyre `PeerId`). Resolve at **accept**, before `OP_HELLO`
  is posted, so the loser dies holding no context. Do **not** tolerate two connections:
  `disconnect` tears down *the* queue pair, so a survivor could still land a write into a
  reclaimed slot (FR-017)
- [X] T082 Tests for T079-T081: **SC-014** — commutativity of version selection over a table of
  range pairs; **SC-015** — two mock endpoints initiating simultaneously converge on one
  connection and agree which *(done in T012, under all four poll orders plus the
  reject-before-request order)*; **SC-016** — a peer at full inbound-request quota does not stop
  this end receiving replies, and two-way traffic at full quota makes progress rather than
  deadlocking; plus one context id live in both directions resolving to two distinct contexts.
  *SC-014 done in slice B* (`negotiation_is_commutative`, every pair of ranges up to 4); the
  SC-016 bookkeeping is unit-tested on `Session`, and its end-to-end form needs slice C's
  asynchronous serving. *SC-016 end to end done in slice C*: both ends fetching from each
  other at three times the credits all complete, and the mock's RNR-stall count stays flat
  once both ends are ready — with the reply reserve not posted it rises (24 stalls)
- [X] T030 [US1] Post receive buffers **unconditionally** on every connection, whether or
  not the local caller uses messaging (FR-005). Derive `max_entries` from the buffer size
  rather than configuring it separately: `(8192 - 24) / 24` = **340** entries per
  `OP_FETCH`, and that is the size the requester chunks to (T044). Post
  **reserve + quota** buffers (T080): one per own message credit for replies, plus the
  inbound-request quota advertised to the peer — 8 × 8 KiB = 64 KiB per peer at the
  defaults of 4 and 4 (sized in research D10), which is why this can be unconditional
  instead of negotiated
- [X] T031 [US1] Implement the context tables in `src/endpoint.rs`: the per-connection
  outbound book `context_id → (fetch, chunk)` and the per-fetch chunk list (offset, entries,
  send time). Allocate ids per connection, match replies by echoed id. A context is one request **message**; a chunked fetch
  (T044) is several contexts, one per chunk, grouped by the requester under the single
  `TransferContextId` that `fetch` returned
- [X] T032 [US1] Implement **message credits** (second domain of FR-014): decrement on
  every request (`OP_FETCH` and `OP_HELLO`), increment on its one reply (success reply or
  `OP_ERROR`), and **block at zero**. A connection starts at one credit, for `OP_HELLO`
  (T076). Credits are consumed per *message*, so a fetch chunked into N messages takes N.
  *Done* (slice B) in `src/session.rs`: a per-connection `Session` with the bootstrap credit,
  the outbound limit `min(message_credits, peer's quota)`, and blocking — a request without
  a credit waits, never fails. Data waits for the peer's `OP_HELLO` **and** our
  `OP_HELLO_REPLY`. Receives (reserve + quota) are posted from a registered `MsgPool` of 8
  KiB buffers as a link comes up (T030, T080), and re-posted after each delivery; the hello
  exchange negotiates a commutative version and fails the connection on no overlap, a zero
  quota or a zero `max_entries` (T076, T077). A reply nobody asked for, an unparseable
  reply, or a request past the advertised quota tears the connection down; an unknown
  opcode is answered with `OP_ERROR` and the connection survives
- [X] T033 [US1] Implement `fetch(peer, items, on_complete)`: send the request, return
  the `TransferContextId` as soon as it is sent, fire `on_complete` once — `Ok` with one
  outcome per requested key when the last reply arrives, `Err` when no usable reply can. The context id exists to correlate a
  `Deadline` error or a log line with its request — it is explicitly **not** a
  cancellation handle
- [X] T034 [US1] Enforce the reply **ordering guarantee** on the source: every RDMA write
  for an entry reported as a hit has completed before the reply is sent. This is what
  makes the one-sided write safe with no `WRITE_WITH_IMM` and no per-entry completion
  notification, so the requester can treat reply arrival as the completion signal
- [X] T035 [US1] Make reserve → arrive → commit/abort possible for the **caller**, who owns
  the landing slots and publishes them: the endpoint never reports a hit for an entry whose
  write did not complete, so a `Miss`, a `Failed` or a teardown is what tells the caller to
  abort that slot, and a hit is safe to commit. Never let a partially written entry read as a
  hit (FR-018, invariant 7). The endpoint itself reserves and publishes nothing
- [X] T036 [US1] Tests: codec round-trip including unknown-opcode rejection and
  unknown-outcome-as-Miss; **SC-001** — a fetch over the mock seam with the zyre receptacle
  absent entirely (the component has no zyre dependency, FR-004, so this is structural)
  and a message counter proving no third peer was contacted; **SC-006** — a request for
  keys the peer lacks returns all-`Miss` and generates no further peer traffic
  (invariant 3, D12: a miss must **not** fall back to broadcast — this mechanism is the
  alternative to broadcast, not a layer above it).
  *Done* (slice C). `fetch` sends one `OP_FETCH` per call once the session is ready and a
  message credit is free (more fetches than credits wait, never fail), refusing anything
  larger than the **peer's** `max_entries`; the source serves it with Phase 3's push path —
  one lookup per entry, pins in the slab, release by the reaper — and replies **once, after
  every write's completion** (T034, `Finish::Reply`). A hit therefore means the write
  landed, so the caller may commit it; anything else, abort (T035). An `OP_ERROR` fails the
  one fetch whose context **and** kind it names, the connection staying up (T075, SC-012);
  one naming another kind, a reply for no request of ours, or a reply of the wrong length is
  a bad reply and tears down. A fetch lost to a teardown fails only after the barrier. Tests:
  SC-001 (values land from the one named peer; a third endpoint is never contacted; no zyre
  receptacle exists), SC-006 (all-miss, one request, no follow-up), T034 (no reply while
  the write is held in flight), SC-012 from both sides, the quota teardown end to end, the
  same context id live both ways, message credits blocking, and the size refusals

**Checkpoint**: SC-001 and SC-006 — US1 is functional end to end over the mock seam. This
is the MVP.

---

## Phase 6: User Story 3 — serving an entry demoted to SSD (Priority: P3)

**Goal**: plan.md Phase 4. Completeness, not a hot path — measured runs show single-digit-
percent SSD hit rates. Requires the `spdk` feature. Spans three components, because reading
an extent belongs to the dispatcher and the bounce reserve to the memory tier (research D13,
D15); each component's own specs are revised with its code.

**Independent Test**: force a key to the block tier, request it, assert the value arrives,
the reply says `hit_block_tier`, and the source's memory-tier occupancy and the relative LRU
order of its memory-tier entries are unchanged.

**Gate**: **SC-005**.

- [X] T083 [US3] `interfaces`: `BounceBuffer { ptr, len }` and `BounceError { NoSpace,
  NeverFits, Io, NotInitialized }`; `IMemoryTier` gains `set_bounce_reserve`,
  `bounce_alloc`, `bounce_free`, `bounce_usage`; `IDispatcher` gains `read_to_bounce` and
  `release_bounce` (contract *The dispatcher's half*). `define_interface!` has no default
  methods, so every implementor changes in the same commit: test doubles answer
  `NeverFits`/no reserve, and `dispatcher-p2p` answers `NeverFits` until it implements the
  path. `EndpointConfig` gains `block_readers` (default 4); `TransferStats` gains
  `block_reads_attempted`, `_completed`, `_failed` and `block_deferrals`. The endpoint's
  `spdk` receptacle becomes `dispatcher: IDispatcher`; no block-device receptacle remains
- [X] T037 [US3] `memory-tier`: the bounce reserve (D15). At `initialize`, set aside
  `min(bounce_reserve, pool_size / 8)` — default 32 MiB — at the top of the pool, managed by
  a second instance of the tier's first-fit free list; `capacity()` excludes it, `used()`
  and eviction tracking never see it, and the initialize log line states its size.
  `set_bounce_reserve` after `initialize` is an error. `bounce_alloc` answers `NoSpace` when
  the request does not fit now and `NeverFits` when it never can: larger than the reserve,
  no reserve, or a pool that is not DMA-capable (the `mmap` fallback). Tests in the memory
  tier: capacity shrinks by exactly the reserve, a full cache cannot reach reserve bytes,
  allocation and coalescing, both errors, the `pool_size / 8` cap
- [X] T038 [US3] `dispatcher`: `read_to_bounce(key, offset, len)` — allocate from the memory
  tier, then read `len` bytes (rounded up to the block size) at `offset` on the drive the
  placement hash chooses for `key`, over a channel leased from that drive's pool, segmented
  by its maximum transfer, exactly as the promotion read is — but into the buffer, and with
  **no device timeout**: the actor's timeout would hand back a buffer the drive may still
  write, so the read passes a timeout that cannot fire, named as a constant whose comment
  says why. On a device error the buffer is returned and `Io` reported. It neither pins nor
  releases: the caller's pin holds the extent still. `release_bounce` forwards to the
  memory tier. Never promotes, never touches the cache (FR-012, D8). Tests over the
  dispatcher's mock block device: the right drive and LBA are read, multi-segment entries,
  `Io` returns the buffer, `NoSpace`/`NeverFits` pass through, the cache is untouched
- [X] T039 [US3] Release the block-tier pin **as soon as its read completes** — before the
  write is posted — since the data has been copied, so only the buffer needs protection
  thereafter (FR-011, D7). The pin waits in the endpoint's read table while its read is in
  flight (nothing is posted yet, so it has no `wr_id`); after the read, the slab carries the
  **bounce buffer**, and the reaper that owns the write's `wr_id` returns it with `release_bounce`.
  This is the one place a pin leaves other than through a completion, and it is stated as
  such
- [X] T040 [US3] Report `HitBlockTier`, and **never promote** the entry into the source's
  memory tier to serve a peer (FR-012, D8). Promotion-to-serve would evict the source's own
  working set, making serving peers self-harming. Structurally: the endpoint calls only
  `read_to_bounce`, which never promotes
- [X] T041 [P] [US3] Without the `spdk` feature there is no dispatcher receptacle, so a
  `BlockDevice{offset}` answer is reported as **`Miss`** — never `HitBlockTier`, never an
  error (D14, invariant 2). Release the pin that lookup took, since that answer *was*
  pinned (contrast T023)
- [X] T072 [US3] Remove the read/write phase barrier from the serve path (FR-022, D19):
  resolve and post **memory-tier** writes immediately, without waiting on any device read,
  and post each block-tier write when its *own* read completes. Reads run on
  `block_readers` threads calling `read_to_bounce`, so the endpoint thread never blocks on a
  device. An entry is looked up only when a send credit is free; a block-tier answer with no
  reader free, or a read answering `NoSpace`, **releases its pin and re-queues the entry**,
  retried when a reader or buffer frees — it never holds a pin while it waits, so pins stay
  within send credits plus readers (SC-004). `NeverFits` and `Io` are `Failed`, counted and
  logged, never retried. A connection torn down mid-read waits for the read: its pin and
  buffer are released when it completes, and the entry is `Failed`. `shutdown` waits for
  every read in flight, however long the device takes
- [X] T073 [US3] Test **SC-011**: on a request mixing memory-tier and block-tier entries,
  every memory-tier write is posted before the first device read completes (hold the mock
  dispatcher's read and assert the memory-tier writes already went out), and the send queue
  is never idle while unposted entries remain
- [X] T042 [US3] Tests: **SC-005** — serving a block-tier entry leaves memory-tier occupancy
  and the relative LRU order of memory-tier entries unchanged; outcome honesty —
  `HitBlockTier` only when the read actually ran (invariant 2); the pin released before the
  write is posted, on one recorded sequence; the buffer returned after the write completes;
  deferral under `NoSpace` and under a busy reader pool, both completing; SC-004 with
  readers; `NeverFits` and `Io` as `Failed`; a teardown and a shutdown mid-read; and the
  no-`spdk` degradation path of T041.
  *Done*, in four commits: `interfaces` (T083, every implementor stubbed as having no
  reserve), `memory-tier` (T037), `dispatcher` (T038), and this crate. Here: `bounce.rs`
  runs `block_readers` threads over a `BlockSource` — `IDispatcher` in an `spdk` build, a
  test double otherwise, so the read path is tested in the default build. A block-tier
  answer with a source becomes `Resolved::Block`; its pin waits in the actor's read table
  (`reads`) and is released when the read returns, before the write is posted; the slab then
  carries `Hold::Buffer`, returned by the reaper (`settle`), which reports `HitBlockTier` only
  on a successful completion. Ready block writes post ahead of new lookups. No reader free,
  or `NoSpace`: release the pin, defer, stop looking up for that pass; deferred entries
  retry as readers free — but not while the reserve is known full, which only a returned
  buffer ends. A `NoSpace` is believed only if nothing was freed after its read was submitted:
  without that, the last buffer can come back before the `NoSpace` it answers is collected,
  and the deferred entries wait forever (the first version of the reserve test caught
  exactly that, as a reply deadline). A torn-down push is marked dead: its reads finish and
  are discarded. `shutdown` waits for every read. Tests (default build, over `DiskSource`):
  US3 end to end; pin after read, buffer after write, on one sequence; SC-011 under a held
  read; SC-004 with 40 entries over 2 credits + 2 readers; a one-buffer reserve deferring
  and completing; `NeverFits`/`Io` as `Failed`, not retried; teardown and shutdown
  mid-read; no source is a `Miss`; SC-005's cache bytes and single lookups; and (`spdk`)
  the `dispatcher` receptacle end to end. SC-005 asserts what is met — the cache's bytes
  untouched and nothing but one lookup asked of the map — since the `LedgerMap` double
  keeps no LRU order to compare

**Checkpoint**: SC-005 — block-tier serving works and is provably tier-neutral.

---

## Phase 7: Bounds and chunking

**Purpose**: plan.md Phase 5. Principle X: every resource consumed on behalf of a peer is
bounded, the bounds are separately named, and exhaustion causes chunking rather than
failure.

**Gate**: **SC-007**.

- [X] T043 Implement **bounce-buffer occupancy** as the third named domain — reserve bytes
  allocated (T037) and readers busy (T072) — and expose all three — send-queue credits (T014),
  message credits (T032), bounce occupancy — as
  separately configured and separately named knobs (FR-014, D10). Do not conflate them:
  they bound genuinely different resources and exhaust independently, and the existing code
  already uses "credits" for the first
- [X] T044 Implement requester-side chunking when `items.len() > max_entries` (the peer's):
  each chunk is its own `OP_FETCH` with **its own `context_id`**, and the requester records,
  per chunk id, the caller's fetch and the chunk's offset in its item list. Replies to the
  chunks of one fetch may arrive in any order (FR-013); each is placed by its chunk's
  recorded offset. Nothing on the wire links the chunks
- [X] T045 Source side: answer every `OP_FETCH` message with **exactly one** reply covering
  all its entries (or one `OP_ERROR`), echoing that message's `context_id` — never one reply per batch
  (FR-007). When bounce space is scarce, serve the message's entries as buffers free up
  and still reply once: scarcity delays the reply, it never multiplies it. This is what keeps
  T080's reply reserve exact
- [X] T046 Implement the fetch completion rule: a fetch is complete when every one of its
  chunks has been answered (one reply per chunk, FR-007), and the outcome counts sum to the
  number of entries requested. `on_complete` fires once, after the last chunk, with the full outcome vector
  in request order — invariant 1 must hold *across* chunking
- [X] T047 Chunk on exhaustion of any of the three domains, and make progress rather than
  failing (FR-015, invariant 9). A request can reach ~3.4 GiB (~1,536 blocks × 2.25 MiB
  for Qwen3-8B at 24k tokens), which is why chunking is mandatory, not optional
- [X] T048 Tests: **SC-007** — with bounce space, message credits and the peer's
  `max_entries` made artificially scarce, a large request still completes, in chunks;
  outcome order preserved across chunk boundaries; and a chunk waits (never errors) at zero
  message credits — `fetch` never blocks the caller and never fails for want of a credit.
  Landing slots are the caller's to reserve, and it requests only what it reserved, so
  their scarcity is not the endpoint's to test.
  *Done* (with T043-T047). `fetch` takes any length and has no size or would-block error.
  Once the peer's `OP_HELLO` is in, a
  fetch is split into chunks of its `max_entries`, in request order; each chunk is a
  message with its own `context_id`, its own credit, its own send time. The outbound book
  maps `context_id` → (fetch, chunk); a reply is checked against its chunk's length and
  placed at its offset, so replies may arrive in any order. Fetches take turns a chunk at
  a time, starting after whichever sent last. A refused chunk marks the fetch refused and
  its unsent chunks settled; once every chunk is settled it fails whole with `Protocol`
  (decision (a)). The deadline runs per chunk and names it. T043 and T045 needed no code:
  the three domains were already separate knobs and counters, and the source already
  replies once per message — scarcity delays the reply (Phase 6's deferral). T047 is the
  sum. Tests: 1,536 entries in five chunks over two credits, byte-exact and in order;
  reverse-order replies placed by offset; a chunk reply of the whole fetch's length is a
  bad reply; a refused chunk failing the fetch only after its siblings, unsent chunks
  unsent; chunks waiting for credits; a small fetch not starved by a large one (which
  caught a real oldest-first bug); a stalled chunk naming itself; and one reply per
  request from a one-buffer reserve. Nine planted bugs, all caught — five by the endpoint
  aborting on an invariant it checks

**Checkpoint**: SC-007 — scarcity degrades to chunking, never to failure.

---

## Phase 8: Fail-loud deadline

**Purpose**: plan.md Phase 6. Principle XI. Note the asymmetry the research settles:
*omitting* the deadline does not fail loudly — vLLM would defer the one request and keep
serving everything else, leaving one stuck request on a healthy-looking engine. Hence a
failing deadline rather than none.

- [X] T049 Implement destination-side deadline tracking per context: detect a request
  neither fulfilled nor refused. Detection is by **deadline, not connection state** — a
  peer that hangs or partitions with the connection open is invisible to the transport,
  and a closed connection is a separate path that aborts its contexts directly
- [X] T050 Implement the default **fail-loud** action: log the key range, peer, chunk index
  and queue-pair state, then fail the request with `Deadline(..)` carrying that diagnostic
  state (FR-016, invariant 10). Release the context's resources, and reclaim landing slots
  only behind the T019 teardown barrier — see T070 for the ordering, which is the part that
  is safety-critical rather than diagnostic
- [X] T051 [P] Add the configuration knob for the action, with the degrading alternative
  (report what arrived and let the caller recompute) **deliberately unimplemented** —
  Principle XI requires any degrading alternative to be opt-in and separately configured,
  and the spec defers this one. Selecting it must fail at configuration time with a clear
  message, not silently behave as fail-loud
- [X] T070 Implement the deadline **cleanup ordering** (FR-023, D18), which is where the
  correctness lives: transition the QP to ERROR → drain and discard its completions →
  reclaim the landing slots → allow a fresh connection only on next need. Logging
  queue-pair state (T050) is not a substitute for changing it: a slow-but-live source whose
  write lands in a reclaimed slot corrupts an unrelated entry. Abort every sibling context
  on that connection and log the blast radius; do **not** narrow it by opening extra
  connections per request. Name this destination-side *reply deadline* distinctly from any
  source-side stall cap (the precedent's `STALL_TIMEOUT`, `connection.rs:116`)
- [X] T071 Tests for T070, over the mock seam: a late write from a stalled peer, injected
  after expiry, **cannot** land in a slot that has been reclaimed (assert from the mock's
  recorded QP→ERROR-before-reclaim ordering); sibling contexts on the connection are
  reported aborted, not silently dropped; and a subsequent fetch to the same peer succeeds
  over a fresh connection **without** re-registering the pool, proving the `rkey`
  survived (SC-010, D17)
- [X] T052 Tests: a stalled mock peer with the connection held open produces a
  `Deadline` error carrying all four diagnostic fields; a closed connection instead aborts
  its outstanding contexts and releases each side's own resources; and the deferred
  degrading action is rejected at configuration time.
  *Done.* The completion became `FetchResult = Result<Vec<EntryOutcome>, _>`: a fetch with no
  usable reply fails as a whole (`Deadline`, `Protocol`, `Transport`) and
  every slot is to be aborted; a push still reports per entry, `Err` only if it never
  started. Only a successful completion is a hit — a write that may or may not have landed
  is reported as not landed. `fetch_deadline` (default 5 s, `None` disables) runs from the
  **send**, so a fetch waiting on a credit or the capability exchange cannot expire. The
  endpoint thread checks it when idle; the oldest expired fetch per peer is logged with the
  peer, key range, entry count, chunk and context, the queue-pair state as the device
  reports it, and the sibling count, then torn down through the `disconnect` barrier
  (ERROR → drain → destroy) and only then are callbacks fired — the expired fetch `Deadline`,
  each sibling `Transport` naming it. A later fetch reconnects lazily. `deadline_action:
  ReportPartial` is refused at `configure`. Tests: the stalled peer (all diagnostic
  fields, `deadline_expiries`, ERROR and destroy stamped before both callbacks, a late
  write from the peer refused with the slot untouched), the sibling abort, no deadline
  waits, an unsent fetch outlives 5x the deadline, `ReportPartial` refused, and the
  reconnect through the same handle (SC-010). The closed-connection case was already
  covered by slice C's teardown test

**Checkpoint**: a hang is loud, and a plausible-but-wrong measurement is not reachable.

---

## Phase 9: Hardware follow-up (deferred; hardware-gated) 🔌

**Purpose**: the real verbs path behind the seam proven in Phases 2–8. Every test here is
`#[ignore]`d so a default `cargo test` never needs a NIC (Principle VIII).

- [X] T053 Implement the real `rdma`-gated transport behind the T010 seam using T009's
  wrappers: `rdma_listen`/`rdma_accept`/`rdma_connect`, with the accepted and the
  initiated QP equally able to post (FR-001). Both predecessor crates already have all
  three calls, so this is combination rather than new ground. **Includes T069**: the seam
  promises a registration outlives every queue pair (T020 relies on it, and the mock keeps
  it); the verbs transport keeps it with one PD per device, as T069 describes
- [X] T054 [P] Write `src/loopback_test.rs`: `#[ignore]`d single-host RDMA loopback
  covering a full `fetch` — request, source push, reply, publication — mirroring the
  pattern of `remote-lookup-rdma-initiator/src/loopback_test.rs`
- [X] T055 [P] Add an `#[ignore]`d loopback test for the legacy `push` path, reproducing
  the scenario of the initiator's `loopback_push_writes_into_remote_buffer`, so SC-002 is
  discharged on hardware and not only over the seam
- [X] T056 Validate on two nodes across the RoCE fabric with GPU↔NUMA↔instance pinning as
  the rigs require, and record the run under `evidence/`.
  *Done* (T053-T056, with T069). `src/connection/verbs.rs` is the verbs transport behind
  the seam, and `select_transport(BuildDefault)` returns it in an `rdma` build — never the
  mock. The listening id is bound at `listen`, which the endpoint calls before registering
  anything, and the one protection domain is made on its device: every region goes into it
  and outlives every queue pair, and a connect that resolves to another device fails (one
  device per endpoint). The event channel is non-blocking (a C shim sets `O_NONBLOCK`), and
  an outbound connect is a state machine — resolve address, resolve route, connect —
  advanced by events. One CQ per link, sized to both queues. Hosts are IPv4 literals.
  `retry_count` and `rnr_retry_count` are 7 (RNR: retry forever, which the receive
  partition makes safe); no RDMA reads, so no responder resources. Hardware tests, all
  `#[ignore]`d: both ends send and write; ERROR flushes receives and a late write is
  refused (`RETRY_EXC_ERR` after ~14 s at rdma_cm's default ACK timeout — it never lands);
  a reject carries its reason; a registration outlives two connections; a connect to nobody
  fails as an event; one poll reaps a burst. Loopback through the interface (T054, T055):
  a fetch round trip and a three-chunk fetch, the legacy push of 64 writes back to front,
  a deadline against a live-but-silent source with the late write not landing and the same
  handle reconnecting, and a lost source. T056 ran node2 ↔ node5 at ~19 GB/s steady for
  96 MiB fetches of 64 KiB entries — `evidence/t056-two-node.md`

**Checkpoint**: `cargo test -- --ignored` passes on an RDMA box; CI is unaffected.

---

## Phase 10: Benchmarks, documentation and polish

- [X] T057 [P] Criterion benchmark in `benches/` for the **per-entry push path** —
  required by plan.md and Principle IV, and the per-entry posting of FR-008 is precisely
  the path whose cost needs measuring. Copy the dev-dependency setup from the responder
  crate
- [X] T058 [P] Criterion benchmark for the message codec, measured inside the real path,
  not alone: a 340-entry `fetch` that misses every entry writes nothing, so what remains is
  the request's encode, the source's decode and resolve, the reply's encode and decode, and
  the credits and thread hops between; an all-hit fetch of the same shape beside it
  attributes the writes
- [X] T059 [P] Doc comments with **runnable** examples on every public item (Principle
  III), using the initiator's `lib.rs` example as the model; `cargo test --doc -p
  certus-transfer-endpoint` passes
- [X] T060 [P] Write `README.md`: role, the symmetric-endpoint property, the three credit
  domains, the feature matrix, and the default-member note from T003
- [X] T061 `cargo doc -p certus-transfer-endpoint --no-deps` warning-free (Principle III)
- [X] T062 `cargo fmt --check` and `cargo clippy -p certus-transfer-endpoint
  --all-features -- -D warnings` (Principle V); confirm every `unsafe` item carries a
  `// SAFETY:` comment
- [X] T063 Run `/component-check-leakage` and confirm the component is reached only through
  its interfaces (Principle I)
- [X] T064 [P] **Design-document hygiene — DONE.** Fixed before the spec set was committed:
  (a) `research.md`'s "Open questions for the plan phase" heading, which was immediately
  followed by D13/D14/D15 while stranded `OQ3`/`OQ4`/`OQ5` bullets sat at the bottom after the
  upstream-change section — reframed as "Plan-phase questions, resolved", recording OQ4 as
  resolved by `contracts/itransfer_endpoint.md` (`PushStatus` widens to `EntryOutcome`,
  collapsible at the boundary at cutover), dropping the duplicated OQ3 (resolved by D15), and
  moving the genuinely deferred half of OQ5 under a labelled "Deferred to the next layer up"
  heading while noting that framing and receive-buffer sizing are now settled in
  `contracts/wire-protocol.md`; (b) `plan.md`'s Constitution Check, which called the
  constitution "an **unfilled template**" although it was ratified v1.0.0 on 2026-10-14 with
  11 principles — now names VIII–XI as the binding gates and maps each to the FRs and
  decisions it restates; (c) `plan.md`'s "Next Step", which still asked for the contracts and
  `tasks.md` to be written — now points at T001/T002 and records the T002 blocker; (d) both
  `spec.md` references to a "T034 hardware loopback test", a name that matches nothing —
  `T034` is a `test_client.rs` error-handling task in the initiator's tasks.md and the
  fmt/clippy task in the responder's — replaced with the real test,
  `remote-lookup-rdma-initiator/src/loopback_test.rs::loopback_push_writes_into_remote_buffer`,
  which T055 reproduces

  *Done* (T057-T063). Both benchmarks go through `ITransferEndpoint` only, as a caller
  would, over the build's own transport — the mock, or verbs with `CERTUS_RDMA_TEST_IP`
  (skipped without it, never substituted). No internals are exported for them: a benchmark
  that called the codec directly would measure something no caller runs. Results and the
  `ib_write_bw` comparison are in `README.md`: a 340-entry all-miss fetch costs ~30 µs over
  verbs; loopback pushes run at 9.6-10.1 GiB/s against `ib_write_bw`'s 12.9-15.0 GiB/s on
  the same loopback, and two nodes reach the fabric's rate (T056). One runnable example on
  `CertusTransferEndpoint` (`create` → wire → `initialize` → `push` → `shutdown`), compiled
  but not run in an `rdma` build, where it would need real addresses. Every `unsafe` block
  and impl now carries `// SAFETY:` (two test sites lacked one). `/component-check-leakage`:
  no crate depends on this one yet (cutover is Phase 11) and this one depends on no other
  component's struct — clean.

**Checkpoint**: the component meets every constitutional gate and the design documents
agree with themselves.

---

## Phase 11: Cutover and retirement (OUT OF SCOPE — do not start)

plan.md Phase 7. Listed for completeness only: **FR-020 explicitly forbids** modifying
`remote-lookup` as part of delivering P1, and no P1 success criterion depends on any of
this. It is a separate change, after the engine is proven.

- [ ] T065 [US2] Re-point `remote-lookup`'s receptacle at `ITransferEndpoint`, collapsing
  `EntryOutcome` to `PushStatus` at the boundary if the widening is unwanted (the legacy
  caller does not use the tier information)
- [ ] T066 [US2] Delete `components/remote-lookup-rdma-initiator` and
  `components/remote-lookup-rdma-responder`; remove them from workspace `members`
- [ ] T067 Specify the shared-memory queue message from the connector into the destination
  endpoint — the next layer up, deliberately designed last in the bottom-up order
  (transfer mechanism → framing → shm-queue message)

---

## Dependencies & Execution Order

### Phase dependencies

- **Phase 1 Setup**: no dependencies, but **T002 gates T003**, and T003 gates the
  Principle VIII claim for every later phase.
- **Phase 2 Foundational**: depends on Setup. **BLOCKS everything else.**
- **Phase 3 (US2)**: depends on Phase 2. Deliberately first among the stories, per
  plan.md.
- **Phase 4 (pin discipline)**: depends on Phase 3's serve path and on T013. Needs no
  messaging, which is why it precedes Phase 5.
- **Phase 5 (US1)**: depends on Phase 4 — the fetch path must not be built on an
  undisciplined pin.
- **Phase 6 (US3)**: depends on Phase 5 for the `HitBlockTier` outcome to be reportable,
  and on the `spdk` feature.
- **Phase 7 (bounds)**: depends on Phases 5 and 6, since two of the three domains are
  introduced there.
- **Phase 8 (deadline)**: depends on Phase 5's context table.
- **Phase 9 (hardware)**: depends only on the T010 seam contract; do it after the MVP.
- **Phase 10 (polish)**: after the phases you intend to ship.
- **Phase 11**: out of scope.

### Story dependencies

- **US2 (P2)**: after Foundational. Independent, and intentionally delivered first.
- **US1 (P1)**: after Foundational + Phase 4. Shares `src/serve.rs` with US2.
- **US3 (P3)**: after US1. Touches `interfaces` (T083), `memory-tier` (T037) and
  `dispatcher` (T038) in that order, then the endpoint's `src/bounce.rs` and its `spdk`
  dispatcher receptacle.

### Parallel opportunities

- Setup: T003 ∥ T004 ∥ T005, all after T002.
- Foundational: T007 (ffi.rs) ∥ T015 (telemetry.rs) ∥ T006 (interfaces crate) — three
  different files. Then T011 ∥ T013 after T010.
- Phase 5: T028 ∥ T029 after T027 — both codecs, one file but disjoint functions; serialise
  if that file is contended.
- Phase 10: T057 ∥ T058 ∥ T059 ∥ T060, and T064 is independent of all code tasks.
- Note: like the predecessor crates, most phases edit `src/lib.rs`, `src/connection.rs` and
  `src/serve.rs`, so run tasks **within** a phase sequentially unless marked `[P]`.

---

## Implementation Strategy

### Engine first, then the MVP

1. Phase 1 → Phase 2: the crate, the single FFI copy, the seam, the symmetric table.
2. Phase 3 (US2): the legacy surface. **STOP and VALIDATE** — SC-002, with
   `remote-lookup` unmodified.
3. Phase 4: pin discipline. **STOP and VALIDATE** — SC-004 and the pin-ledger test. This
   is the safety-critical gate; Workflow rule 6 requires an explicit Principle IX review.
4. Phase 5 (US1): verbs messaging. **STOP and VALIDATE** — SC-001 and SC-006. This is the
   MVP and the point at which the research question becomes measurable.

### Incremental delivery

1. MVP (Phases 1–5) → Phase 6 (US3, block-tier) → Phase 7 (bounds) → Phase 8 (deadline).
2. Phase 9 on hardware once the seam-level behaviour is settled.
3. Phase 10 polish; Phase 11 only as a separate, later change.

---

## Traceability

| Requirement | Tasks |
|---|---|
| FR-001 symmetric QP | T012, T053 |
| FR-002 one connection per peer, reused | T012, T021 |
| FR-003 builds with no rdma-core | T001, T003, T010, T011 |
| FR-004 no zyre dependency | T001, T035 |
| FR-005 unconditional posted receives | T030 |
| FR-006 request contents | T027, T028 |
| FR-007 reply contents | T029 |
| FR-008 one write per entry | T017, T057 |
| FR-009 atomic lookup+pin | T022 |
| FR-010 never release an unpinned answer | T023, T026 |
| FR-011 per-entry pin lifetime | T024, T039 |
| FR-012 bounce reserve, no promotion | T083, T037, T038, T040 |
| FR-013 bounded concurrency | T014, T043 |
| FR-014 three credit domains | T014, T032, T043, T072 |
| FR-015 chunk, never fail | T044–T047, T048 |
| FR-016 fail-loud deadline | T049–T052 |
| FR-017 teardown before reclaim | T019, T021, T050 |
| FR-018 no partial publication | T035 |
| FR-019 legacy push entry point | T017, T018 |
| FR-020 `remote-lookup` untouched | Phase 11 (not started) |
| FR-021 instance-granular peer identity | T068, T012 |
| FR-022 no phase barriers | T017, T072, T073 |
| FR-023 deadline cleanup ordering | T070, T071, T019 |
| FR-024 protocol error reply | T074, T075, T078 |
| FR-025 single negotiated version + capability exchange | T076, T077, T078 |
| FR-026 bidirectional connection: namespaces, credits, buffers | T079, T080, T082 |
| FR-027 simultaneous-connect resolution | T012 (absorbed T081), T082 |
| FR-028 a panic on an endpoint or reader thread aborts | T016, T072 |
| SC-001 zero zyre, zero third instance | T035 |
| SC-002 legacy push scenario on hardware, properties over the mock | T021, T055 |
| SC-003 default build, no rdma-core/SPDK | T001, T003, T010, T041 |
| SC-004 pin bound | T025, T072, T042 |
| SC-005 tier neutrality | T040, T042 |
| SC-006 all-miss, no broadcast | T035 |
| SC-007 progress under scarcity | T048 |
| SC-008 FFI defined once | T007 |
| SC-009 two instances per host addressed independently | T068, T012 |
| SC-010 no late write after deadline; rkey survives reconnect | T071 |
| SC-011 no read/write phase barrier | T073 |
| SC-012 unknown opcode answered, connection survives | T078 |
| SC-013 peer max_entries respected, floor fails at connect | T078 |
| SC-014 version rule is commutative | T077, T082 |
| SC-015 one connection survives a race | T012 |
| SC-016 replies not starved by inbound requests | T080, T082 |
| D21 version is a validated header field | T027 |
| D22 one connection, both directions | T079, T080 |
| D23 deterministic race resolution | T012 |
| D24 connect-time identity, name echo, newer wins | T012 |
| D20 one negotiated version, frozen messages, eager discovery | T027, T074-T078 |
| D17 long-lived PD, registrations outlive QPs | T020 (seam), T053 + T069 (verbs) |
| Principle VIII default build | T002, T003, T010, T054–T056 |
| Principle IX pin discipline | T013, T022–T026, T039 |
| Principle X bounded resources | T014, T032, T043, T047 |
| Principle XI loud failure | T015, T050, T051 |

---

## Notes

- Generated from plan.md with
  `SPECIFY_FEATURE_DIRECTORY=components/certus-transfer-endpoint/specs/001-rdma-cache-transfer
  .specify/scripts/bash/setup-tasks.sh`, which resolves this feature directory and the
  root `tasks-template.md`. Specs live **with the component**, not at repo root, so the
  override is required.
- `[P]` = different file, no incomplete-task dependency.
- Tests are in-module (`#[cfg(test)] mod tests`) per both predecessor crates; benchmarks
  live in `benches/`; the hardware loopback test is its own `#[ignore]`d module.
- All verbs FFI is `unsafe` and requires `// SAFETY:` comments.
- Commit after each task or logical group; stop at any checkpoint to validate a phase
  against its success criterion.
- **Amended again 2026-10-15**: protocol versioning settled as **D20** — **one** protocol
  version, negotiated eagerly at connect via a frozen `OP_HELLO`, with a frozen `OP_ERROR` as
  the backstop (FR-024, FR-025, SC-012, SC-013; tasks **T074-T078**). This supersedes
  `wire-protocol.md`'s original "no negotiation" stance, deliberately: lazy
  fallback-on-error would have to re-frame and re-send a request whose landing slots are
  already reserved and whose rkeys are already advertised, which is more machinery than one
  exchange at connect — and the exchange additionally makes `max_entries` explicit, which
  nothing previously checked. Version-*selection* beyond `min()` stays deferred until a v2
  exists.
- **Amended 2026-10-15 after review**, before any transfer logic was written. Four findings
  were folded in: FR-021 (instance-granular peer identity), FR-022 (no phase barriers),
  FR-023 (deadline cleanup ordering), plus D16-D19 and SC-009/010/011. New tasks are
  **T068-T073**, numbered append-only and placed in their phases, so no existing task
  renumbered and no cross-reference broke. T009, T012, T017, T019, T020 and T050 were
  tightened in place. Note D19 is partly a decision *not* to build: cross-message
  pipelining is worth ~0.1% and is deliberately not a task.
- **Discrepancies found while generating this list have been fixed** (T064, marked done):
  `research.md` read as having three open questions when none were open; `plan.md` called the
  constitution an unfilled template and its "Next Step" still asked for work already finished;
  and `spec.md` twice cited a "T034 hardware loopback test" that matches nothing under that
  name. The one finding that is *not* a documentation fix is **T002**: the `IDispatchMap`
  gating blocker, which the plan did not anticipate and which must be cleared before the
  component can be a workspace `default-member` as Principle VIII requires.
