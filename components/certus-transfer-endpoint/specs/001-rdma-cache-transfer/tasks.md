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
- [ ] T004 [P] Declare the module skeleton in `src/lib.rs` per plan.md's layout:
  `mod connection; mod message; mod serve; mod telemetry;`, `#[cfg(feature = "rdma")] mod
  ffi; #[cfg(feature = "rdma")] mod rdma;`, `#[cfg(feature = "spdk")] mod bounce;`, and
  `#[cfg(test)] mod loopback_test;` gated for Phase 9
- [ ] T005 [P] Write the component `CLAUDE.md` (plan.md Constitution Check requires one):
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

- [ ] T006 Define `ITransferEndpoint` in
  `components/interfaces/src/itransfer_endpoint.rs` via `define_interface!`, **ungated**
  (Principle I, contracts/itransfer_endpoint.md): methods
  `register_landing_region`, `fetch`, `push_async`, `push`, `disconnect`, `stats`, plus
  types `EntryOutcome`, `TransferContextId`, `LandingRegionHandle`,
  `TransferEndpointError`, `FetchCompletion`, `TransferStats`. Reuse the existing
  `RemoteRegion` and `CacheKey` verbatim. **No `serve()` method** — serving is reactive
  by design, and exposing it would invite callers to schedule it
- [ ] T007 Write `src/ffi.rs` as **the single copy** of the verbs FFI (SC-008): the 28
  public items currently duplicated across
  `remote-lookup-rdma-initiator/src/ffi.rs` and `remote-lookup-rdma-responder/src/ffi.rs`,
  plus the send/recv surface neither crate ever used — `ibv_post_recv`, the recv work
  request and SGE types, and `IBV_WR_SEND` promoted from a bare unused constant to a
  live opcode (D3). Every `unsafe` item carries a `// SAFETY:` comment (Principle V)
- [ ] T008 Write `build.rs` + `src/wrapper.c` for the static-inline verbs shim, compiled
  by `cc` and located by `pkg-config`, **both gated on `rdma`** so the default build
  invokes neither (Principle VIII). Copy the initiator's shim as the model
- [ ] T009 Write `src/rdma.rs` safe wrappers over T007: device/PD open, QP lifecycle
  (init → RTR → RTS → ERROR), `MemoryRegion` over caller-owned memory (the initiator's
  registration path already covers "memory owned by the caller (e.g. memory-tier pool)",
  D15), `post_send`, `post_recv`, completion polling. Gated on `rdma`. **The protection
  domain is owned by the endpoint for its lifetime, not by a connection** (D17) — unlike the
  initiator, where the PD lives in the connection (`rdma.rs:169`, `ibv_dealloc_pd` in its
  `Drop`) and the pool is re-registered on every connect (`connection.rs:1131`). Explicit QP
  transition to ERROR is part of this surface: it is what FR-023 relies on
- [ ] T010 Define the **transport seam** in `src/connection.rs`: traits for a listener
  and a peer link that carry a QP and can *both* post and receive, with `CmEvent`-style
  inbound delivery. **The seam must be selected per build, with the interface working in
  every build** — do *not* copy the initiator's
  `#[cfg(not(feature = "rdma"))] push_async -> Err(NotInitialized)` pattern, which makes
  the interface untestable without hardware. D14 and contract invariant 2 both require
  the default build to *serve* over the mock seam, so the seam is the implementation of
  the interface, not a bypass of it (Principle VIII, Workflow rule 2 "Seam First")
- [ ] T011 [P] Implement the in-process mock transport in `src/connection.rs` (depends
  T010): two endpoints in one process, injectable inbound messages, a simulated one-sided
  write that copies into the target address, and recorded QP→ERROR ordering so
  teardown-before-reclaim is assertable
- [ ] T012 Implement the **symmetric** `ConnectionTable` in `src/connection.rs` (depends
  T010, T068, D5): one QP per peer **instance** pair keyed by `PeerId`, established on first
  need and reused
  (FR-002), with a connection usable regardless of whether it was inbound-accepted or
  outbound-initiated, and **either end able to post work requests** (FR-001). This is the
  thing neither existing crate can do
- [ ] T013 [P] Implement the `wr_id` slab in `src/connection.rs` (depends T010): allocate
  a slot per posted work request carrying the per-entry state its completion must release
  (pin, bounce lease, context + entry index). This is the mechanism Principle IX requires
  for "released by the completion reaper that owns its work-request id"
- [ ] T014 Implement **send-queue credits** in `src/connection.rs` (depends T012, T013):
  bound in-flight RDMA writes per connection, with partial batch posting and a completion
  reaper that refills. First of the three domains of D10/FR-014; the other two arrive in
  Phase 7
- [ ] T015 [P] Implement `src/telemetry.rs`: ZST no-op with
  `#[allow(clippy::unused_self)]` when `telemetry` is off, `AtomicU64` counters when on,
  identical method surface. Counters: per-outcome totals, each credit domain's current and
  peak depth, chunk count, deadline expiries. Principle XI forbids structuring these so an
  unexercised path looks like a working one — so count *attempts* alongside completions
- [ ] T068 [P] Implement `PeerId` in `src/connection.rs`: parse and normalize a
  `"ip:port"` instance address, rejecting a missing port and port 0 rather than defaulting
  either (FR-021, D16). Reuse the grammar of
  `remote-lookup-rdma-initiator/src/connection.rs:1075` `parse_endpoint` rather than
  inventing a second one. Unit-test that two addresses differing only by port are distinct
  keys, that a host-only address is rejected, and that both may be the local host
- [ ] T069 Implement protection-domain ownership in `src/rdma.rs` (depends T009): **one PD
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
  needed alongside `Send` (Principle V)
- [ ] T016 Wire `define_component!` in `src/lib.rs` (depends T006, T012): receptacles
  `logger: ILogger`, `dispatch_map: IDispatchMap` (both ungated, per D13 and T002), and a
  block-device receptacle gated on `spdk`. Endpoint owns a dedicated actor thread, as both
  predecessor crates do

**Checkpoint**: the engine builds and the mock seam carries a message and a simulated
write end to end, with neither rdma-core nor SPDK present.

---

## Phase 3: User Story 2 — the legacy push path keeps working (Priority: P2)

**Goal**: the messaging-free subset of FR-019, so `remote-lookup` could be re-pointed with
a receptacle change and no logic change — while leaving it untouched (FR-020).

**Independent Test**: the existing `remote-lookup` test layer, plus the initiator's
hardware loopback scenario, pass against this component with messaging unused.

**Gate**: **SC-002**.

- [ ] T017 [US2] Implement `push_async(endpoint, &[(CacheKey, RemoteRegion)],
  on_complete)` in `src/serve.rs` + `src/lib.rs`: resolve or open the connection, post one
  RDMA write **per entry** (FR-008 — landing slots are not contiguous, so there is no
  batch write to post), accumulate outcomes, fire `on_complete` once with one outcome per
  item in request order. Post continuously: refill on each completion and never drain the
  send queue between entries while unposted entries remain (FR-022). A window of 2-4 already
  saturates the link at 2.25 MiB per entry, so this is about avoiding idle, not depth (D19)
- [ ] T018 [US2] Implement blocking `push` over `push_async` (contract: one outcome per
  item, in order)
- [ ] T019 [US2] Implement `disconnect(peer)`: transition the peer's QP to ERROR, drain and
  discard its completions, and do not return until no write from that peer can still land
  (FR-017, Principle IX's last clause). This is the existing `Disconnect`/`DisconnectAck`
  barrier, kept — and it is also the mechanism the deadline path reuses (FR-023, T070), so
  it must abort every context outstanding on the connection, not just one
- [ ] T020 [US2] Implement `register_landing_region(base, len)` returning
  `LandingRegionHandle`: register once, not per request; an `rkey` is only valid because
  of it (wire-protocol.md "Connection and lifecycle"). Register into the long-lived PD of
  T069 so the handle and its `rkey`s survive a reconnect (D17); depends on T069
- [ ] T021 [US2] Tests in-module: order-and-length invariant 1 over the mock seam
  (`items.len()` outcomes, index *i* ↔ `items[i]`); connection reuse across repeated
  pushes (FR-002); teardown-before-reclaim ordering asserted from the mock's recorded
  QP→ERROR sequence (invariant 8); and a `#[test]` that the default build serves a
  memory-tier entry through the **interface**, not merely through `connection`

**Checkpoint**: SC-002 — the legacy surface is complete and tested with zero messaging,
and `remote-lookup` has not been modified.

---

## Phase 4: Pin discipline (cross-cutting)

**Purpose**: plan.md Phase 2. Verifiable with no messaging at all, which is why it
precedes Phase 5. Principle IX is the review gate; changes here "MUST be reviewed against
Principle IX explicitly" (Workflow rule 6).

**Gate**: **SC-004**.

- [ ] T022 [XC] Implement the per-entry serve step in `src/serve.rs`: **one**
  `dispatch_map.lookup(key)` yields residency *and* the pin *and* the address
  (`MemoryTier{pointer,size}`) or the extent offset (`BlockDevice{offset}`) together
  (FR-009, D6). Do not use `Check` to pick the tier first — it releases immediately and
  would reintroduce exactly the probe-then-pin window the dispatch map is built to avoid
- [ ] T023 [XC] Never release a read reference for an unpinned answer: `NotExist` and
  `MismatchSize` return before the pin, so releasing on them decrements another reader's
  pin (FR-010, D6 corollary, Principle IX). Treat `MismatchSize` as `Miss` (spec edge
  case: "key present but stored size does not match"). Note this is the live
  `Dispatcher::check()` defect — `exists = !matches!(result, NotExist)` then release —
  which is tracked separately and must **not** be reproduced here
- [ ] T024 [XC] Release a memory-tier pin from the completion reaper that owns its
  `wr_id`, per entry, never per batch (FR-011, D7, depends T013)
- [ ] T025 [XC] Test **SC-004**: with send-queue credits set to *n*, a push of
  *n* × 100 entries never holds more than *n* concurrent pins. Instrument the mock
  dispatch map to record peak concurrent `read_ref`, and assert the bound independently of
  `items.len()` (invariant 5)
- [ ] T026 [XC] Test the double-release hazard directly: a request mixing hits,
  `NotExist` and `MismatchSize` leaves every key's `read_ref` at exactly its starting
  value. An over-release corrupts an unrelated reader and no ordinary test catches it —
  Principle IX's rationale asks for this one specifically

**Checkpoint**: SC-004 holds, and the pin ledger balances across mixed outcomes.

---

## Phase 5: User Story 1 — destination fetches a prefix from a named peer (Priority: P1) 🎯 MVP

**Goal**: plan.md Phase 3. The verbs request/reply layer that replaces zyre broadcast with
one explicit point-to-point ask.

**Independent Test**: two endpoints over the mock seam; requested keys land in the named
slots and the reply's per-key outcomes match the source's actual residency.

**Gate**: **SC-001** — zero zyre messages, zero messages to any third node.

- [ ] T027 [US1] Implement the 24-byte header codec in `src/message.rs`: fused
  `opcode = (version << 16) | kind`, `OP_FETCH = 0x0001_0001`,
  `OP_FETCH_REPLY = 0x0001_0002`; `flags` bit 0 = `MORE`; `context_id`; `n_entries`;
  reserved fields that MUST be zero. Little-endian. Dispatch on the **whole word** and
  reject any unknown value — the fusing is what makes an unknown version structurally an
  unknown opcode rather than a misparse
- [ ] T028 [P] [US1] Implement the 24-byte entry codec: `(key u64, addr u64, size u32,
  rkey u32)` — exactly `(CacheKey, RemoteRegion)`. **Absolute addresses, not slot
  indices**: `IMemoryTier` is an allocator keyed by `CacheKey` with per-entry size and has
  no index-addressable slot table, so indices would invent a layout the tier does not
  have. The per-entry `rkey` rides in what would otherwise be alignment padding and lets
  one request land across several registered regions
- [ ] T029 [P] [US1] Implement the reply codec: one outcome byte per entry in request
  order (`0` Miss, `1` HitMemoryTier, `2` HitBlockTier), zero-padded to an 8-byte
  boundary. A receiver MUST treat an unknown outcome (≥ 3) as `Miss`, never as an error,
  so a future source can add a tier without breaking an old requester
- [ ] T030 [US1] Post receive buffers **unconditionally** on every connection, whether or
  not the local caller uses messaging (FR-005). Derive `max_entries` from the buffer size
  rather than configuring it separately: `(8192 - 24) / 24` = **340** entries per
  `OP_FETCH`, and that is the value `RequestTooLarge { max_entries }` reports. Posted-recv
  memory is `message_credits × 8 KiB` — 64 KiB per peer at 8 credits, which is why this
  can be unconditional instead of negotiated
- [ ] T031 [US1] Implement the context table in `src/message.rs`: `TransferContext` =
  context id, peer, key list, landing slots, outstanding-entry count, deadline. Allocate
  ids, match replies by echoed id
- [ ] T032 [US1] Implement **message credits** (second domain of FR-014): decrement on
  send, increment on reply, and **block at zero**. Credits are consumed per *message*, not
  per context, so a chunked fetch of N messages takes N credits
- [ ] T033 [US1] Implement `fetch(peer, items, on_complete)`: send the request, return
  the `TransferContextId` as soon as it is sent, fire `on_complete` once with one outcome
  per requested key when the last reply arrives. The context id exists to correlate a
  `Deadline` error or a log line with its request — it is explicitly **not** a
  cancellation handle
- [ ] T034 [US1] Enforce the reply **ordering guarantee** on the source: every RDMA write
  for an entry reported as a hit has completed before the reply is sent. This is what
  makes the one-sided write safe with no `WRITE_WITH_IMM` and no per-entry completion
  notification, so the requester can treat reply arrival as the completion signal
- [ ] T035 [US1] Implement reserve → arrive → commit/abort on the destination: publish
  only entries whose write completed, release the rest; never publish a partially written
  entry (FR-018, invariant 7)
- [ ] T036 [US1] Tests: codec round-trip including unknown-opcode rejection and
  unknown-outcome-as-Miss; **SC-001** — a fetch over the mock seam with the zyre receptacle
  absent entirely (the component has no zyre dependency, FR-004, so this is structural)
  and a message counter proving no third peer was contacted; **SC-006** — a request for
  keys the peer lacks returns all-`Miss` and generates no further peer traffic
  (invariant 3, D12: a miss must **not** fall back to broadcast — this mechanism is the
  alternative to broadcast, not a layer above it)

**Checkpoint**: SC-001 and SC-006 — US1 is functional end to end over the mock seam. This
is the MVP.

---

## Phase 6: User Story 3 — serving an entry demoted to SSD (Priority: P3)

**Goal**: plan.md Phase 4. Completeness, not a hot path — measured runs show single-digit-
percent SSD hit rates. Requires the `spdk` feature.

**Independent Test**: force a key to the block tier, request it, assert the value arrives,
the reply says `hit_block_tier`, and the source's memory-tier occupancy and LRU order are
unchanged.

**Gate**: **SC-005**.

- [ ] T037 [US3] Implement `src/bounce.rs`: a fixed pool of buffers allocated with
  `spdk_zmalloc` on the serving instance's NUMA node and registered with
  `MemoryRegion` over caller-owned memory — **both** registrations, because the device
  read needs SPDK-DMA-able memory and the RDMA needs `ibv_reg_mr`. The memory tier is
  already dual-registered exactly this way, which is why no new mechanism is needed.
  `gpu-services::prepare_memory_for_spdk` is **not** used; it addresses GPU memory (D15)
- [ ] T038 [US3] Implement the device read over `connect_client() -> ClientChannels`
  (the actor-per-controller model with shared-memory client channels, per the repo
  `CLAUDE.md`), not direct trait methods: `lookup` → `BlockDevice{offset}` (already
  pinned) → read that offset into a leased bounce buffer (D13)
- [ ] T039 [US3] Release the block-tier pin **as soon as the device read completes** —
  before the RDMA write is posted — because the data has been copied, so only the buffer
  needs protection thereafter (FR-011, D7). The bounce lease, not the pin, is what the
  `wr_id` completion then releases
- [ ] T040 [US3] Report `HitBlockTier`, and **never promote** the entry into the source's
  memory tier to serve a peer (FR-012, D8). Promotion-to-serve would evict the source's own
  working set, making serving peers self-harming
- [ ] T041 [P] [US3] Without the `spdk` feature there is no block-device receptacle bound,
  so a `BlockDevice{offset}` answer is reported as **`Miss`** — never `HitBlockTier`, never
  an error (D14, invariant 2). Release the pin that lookup took, since that answer *was*
  pinned (contrast T023)
- [ ] T072 [US3] Remove the read/write phase barrier from the serve path (FR-022, D19):
  resolve and post **memory-tier** writes immediately, without waiting on any device read,
  and post each block-tier write when its *own* read completes — never "lease N buffers,
  read all N, post all N". Keep the device queue deep by sizing bounce occupancy for it,
  which is the one bound that should track device parallelism. This is the only barrier in
  the design whose cost is milliseconds rather than microseconds, which is why it is the
  only one with a requirement attached
- [ ] T073 [US3] Test **SC-011**: on a request mixing memory-tier and block-tier entries,
  every memory-tier write is posted before the first device read completes (hold the mock
  block device's read completion and assert the memory-tier writes already went out), and
  the send queue is never idle while unposted entries remain
- [ ] T042 [US3] Tests: **SC-005** — serving a block-tier entry leaves memory-tier
  occupancy and LRU order byte-identical (invariant 6); outcome honesty — `HitBlockTier`
  only when the device was actually read (invariant 2); and the no-`spdk` degradation path
  of T041

**Checkpoint**: SC-005 — block-tier serving works and is provably tier-neutral.

---

## Phase 7: Bounds and chunking

**Purpose**: plan.md Phase 5. Principle X: every resource consumed on behalf of a peer is
bounded, the bounds are separately named, and exhaustion causes chunking rather than
failure.

**Gate**: **SC-007**.

- [ ] T043 Implement **bounce-buffer occupancy** as the third named domain and expose all
  three — send-queue credits (T014), message credits (T032), bounce occupancy — as
  separately configured and separately named knobs (FR-014, D10). Do not conflate them:
  they bound genuinely different resources and exhaust independently, and the existing code
  already uses "credits" for the first
- [ ] T044 Implement requester-side chunking when `items.len() > max_entries`: chunks of
  one context share a `context_id`, and every chunk but the last sets `MORE`
- [ ] T045 Implement source-side reply chunking: one reply per batch completed, each
  covering a contiguous run of the request's entries, `MORE` on all but the last
- [ ] T046 Implement the context completion rule: a `context_id` is complete when a reply
  without `MORE` has been seen **and** the outcome counts sum to the number of entries
  requested. `on_complete` fires once, after the last chunk, with the full outcome vector
  in request order — invariant 1 must hold *across* chunking
- [ ] T047 Chunk on exhaustion of any of the three domains, and make progress rather than
  failing (FR-015, invariant 9). A request can reach ~3.4 GiB (~1,536 blocks × 2.25 MiB
  for Qwen3-8B at 24k tokens), which is why chunking is mandatory, not optional
- [ ] T048 Tests: **SC-007** — with bounce buffers and landing slots made artificially
  scarce, a large request still completes, in chunks; outcome order preserved across
  chunk boundaries; and a send blocks (not errors) at zero message credits, while
  `WouldBlock` is returned only when the caller explicitly asked not to block

**Checkpoint**: SC-007 — scarcity degrades to chunking, never to failure.

---

## Phase 8: Fail-loud deadline

**Purpose**: plan.md Phase 6. Principle XI. Note the asymmetry the research settles:
*omitting* the deadline does not fail loudly — vLLM would defer the one request and keep
serving everything else, leaving one stuck request on a healthy-looking engine. Hence a
failing deadline rather than none.

- [ ] T049 Implement destination-side deadline tracking per context: detect a request
  neither fulfilled nor refused. Detection is by **deadline, not connection state** — a
  peer that hangs or partitions with the connection open is invisible to the transport,
  and a closed connection is a separate path that aborts its contexts directly
- [ ] T050 Implement the default **fail-loud** action: log the key range, peer, chunk index
  and queue-pair state, then fail the request with `Deadline(..)` carrying that diagnostic
  state (FR-016, invariant 10). Release the context's resources, and reclaim landing slots
  only behind the T019 teardown barrier — see T070 for the ordering, which is the part that
  is safety-critical rather than diagnostic
- [ ] T051 [P] Add the configuration knob for the action, with the degrading alternative
  (report what arrived and let the caller recompute) **deliberately unimplemented** —
  Principle XI requires any degrading alternative to be opt-in and separately configured,
  and the spec defers this one. Selecting it must fail at configuration time with a clear
  message, not silently behave as fail-loud
- [ ] T070 Implement the deadline **cleanup ordering** (FR-023, D18), which is where the
  correctness lives: transition the QP to ERROR → drain and discard its completions →
  reclaim the landing slots → allow a fresh connection only on next need. Logging
  queue-pair state (T050) is not a substitute for changing it: a slow-but-live source whose
  write lands in a reclaimed slot corrupts an unrelated entry. Abort every sibling context
  on that connection and log the blast radius; do **not** narrow it by opening extra
  connections per request. Name this destination-side *reply deadline* distinctly from any
  source-side stall cap (the precedent's `STALL_TIMEOUT`, `connection.rs:116`)
- [ ] T071 Tests for T070, over the mock seam: a late write from a stalled peer, injected
  after expiry, **cannot** land in a slot that has been reclaimed (assert from the mock's
  recorded QP→ERROR-before-reclaim ordering); sibling contexts on the connection are
  reported aborted, not silently dropped; and a subsequent fetch to the same peer succeeds
  over a fresh connection **without** re-registering the landing region, proving the `rkey`
  survived (SC-010, D17)
- [ ] T052 Tests: a stalled mock peer with the connection held open produces a
  `Deadline` error carrying all four diagnostic fields; a closed connection instead aborts
  its outstanding contexts and releases each side's own resources; and the deferred
  degrading action is rejected at configuration time

**Checkpoint**: a hang is loud, and a plausible-but-wrong measurement is not reachable.

---

## Phase 9: Hardware follow-up (deferred; hardware-gated) 🔌

**Purpose**: the real verbs path behind the seam proven in Phases 2–8. Every test here is
`#[ignore]`d so a default `cargo test` never needs a NIC (Principle VIII).

- [ ] T053 Implement the real `rdma`-gated transport behind the T010 seam using T009's
  wrappers: `rdma_listen`/`rdma_accept`/`rdma_connect`, with the accepted and the
  initiated QP equally able to post (FR-001). Both predecessor crates already have all
  three calls, so this is combination rather than new ground
- [ ] T054 [P] Write `src/loopback_test.rs`: `#[ignore]`d single-host RDMA loopback
  covering a full `fetch` — request, source push, reply, publication — mirroring the
  pattern of `remote-lookup-rdma-initiator/src/loopback_test.rs`
- [ ] T055 [P] Add an `#[ignore]`d loopback test for the legacy `push` path, reproducing
  the scenario of the initiator's `loopback_push_writes_into_remote_buffer`, so SC-002 is
  discharged on hardware and not only over the seam
- [ ] T056 Validate on two nodes across the RoCE fabric with GPU↔NUMA↔instance pinning as
  the rigs require, and record the run under `evidence/`

**Checkpoint**: `cargo test -- --ignored` passes on an RDMA box; CI is unaffected.

---

## Phase 10: Benchmarks, documentation and polish

- [ ] T057 [P] Criterion benchmark in `benches/` for the **per-entry push path** —
  required by plan.md and Principle IV, and the per-entry posting of FR-008 is precisely
  the path whose cost needs measuring. Copy the dev-dependency setup from the responder
  crate
- [ ] T058 [P] Criterion benchmark for the message codec (header + entry vector encode and
  decode at 340 entries)
- [ ] T059 [P] Doc comments with **runnable** examples on every public item (Principle
  III), using the initiator's `lib.rs` example as the model; `cargo test --doc -p
  certus-transfer-endpoint` passes
- [ ] T060 [P] Write `README.md`: role, the symmetric-endpoint property, the three credit
  domains, the feature matrix, and the default-member note from T003
- [ ] T061 `cargo doc -p certus-transfer-endpoint --no-deps` warning-free (Principle III)
- [ ] T062 `cargo fmt --check` and `cargo clippy -p certus-transfer-endpoint
  --all-features -- -D warnings` (Principle V); confirm every `unsafe` item carries a
  `// SAFETY:` comment
- [ ] T063 Run `/component-check-leakage` and confirm the component is reached only through
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
- **US3 (P3)**: after US1. Adds `src/bounce.rs` and the `spdk` receptacle.

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
| FR-012 bounce buffers, no promotion | T037, T040 |
| FR-013 bounded concurrency | T014, T043 |
| FR-014 three credit domains | T014, T032, T043 |
| FR-015 chunk, never fail | T044–T047, T048 |
| FR-016 fail-loud deadline | T049–T052 |
| FR-017 teardown before reclaim | T019, T021, T050 |
| FR-018 no partial publication | T035 |
| FR-019 legacy push entry point | T017, T018 |
| FR-020 `remote-lookup` untouched | Phase 11 (not started) |
| FR-021 instance-granular peer identity | T068, T012 |
| FR-022 no phase barriers | T017, T072, T073 |
| FR-023 deadline cleanup ordering | T070, T071, T019 |
| SC-001 zero zyre, zero third node | T035 |
| SC-002 legacy tests pass | T021, T055 |
| SC-003 default build, no rdma-core/SPDK | T001, T003, T010, T041 |
| SC-004 pin bound | T025 |
| SC-005 tier neutrality | T040, T042 |
| SC-006 all-miss, no broadcast | T035 |
| SC-007 progress under scarcity | T048 |
| SC-008 FFI defined once | T007 |
| SC-009 two instances per host addressed independently | T068 |
| SC-010 no late write after deadline; rkey survives reconnect | T071 |
| SC-011 no read/write phase barrier | T073 |
| D17 long-lived PD, registrations outlive QPs | T069, T020 |
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
