# Feature Specification: RDMA Lookup Responder

**Feature Branch**: `rework-remote-lookup-rdma-initiator`

**Created**: 2026-07-10

**Status**: Draft

**Input**: The **accept** side of a remote RDMA lookup, belonging to the
*requesting* Certus instance — the node that wants a value and offers local
memory for a peer to write it into. Passive counterpart of the outbound
initiator (`remote-lookup-rdma-initiator` / `IRemoteLookupRdmaInitiator`). Owned
and driven by the `remote-lookup` component for control, and by the application
mainline for lifecycle.

## Clarifications

### Session 2026-07-10 (responder model)

- Q: Which side of the data path is this? → A: The passive accept side. A serving
  peer connects *in* and RDMA-*writes* one-sidedly into pre-registered local
  memory. The responder's CPU never touches the value bytes; it manages only
  connections (queue pairs).
- Q: How does the responder tie an inbound queue pair to a zyre peer, given that
  the RDMA connect is out-of-band from zyre and co-resident instances share a host
  IP? → A: The serving initiator stamps its own zyre UUID into the `rdma_cm`
  connect `private_data`; the responder reads it on `CONNECT_REQUEST` and keys its
  per-node connection table by `PeerId`. Unstamped connections surface as
  `ConnectionEstablished { node: None }` and are reclaimable only via `shutdown`
  (there is no per-node handle for them).
- Q: Why an ephemeral port rather than a well-known one? → A: One Certus instance
  runs per NUMA domain and co-resident instances may share a single RDMA NIC. The
  responder binds port 0, reads the assigned port back with `rdma_get_src_port`,
  and advertises `{ip, port}` so co-resident instances never collide. The device
  is never pinned by name — binding by IP implies the NIC/NUMA path.
- Q: How is a departing peer's memory reclaimed safely? → A: Teardown-before-
  reclaim. `remote-lookup` issues `Disconnect { node }`; the responder transitions the
  RC queue pair to that peer into the ERROR state (a QP in ERROR NAKs late writes so
  they cannot land) **before** acknowledging with `DisconnectAck { node }`, and only
  then may the requester reclaim the peer's locked landing slots.

### Session 2026-07-10 (clarify)

- Q: If the QP→ERROR transition itself fails during `Disconnect`, what is the
  `DisconnectAck` contract? → A: The ERROR-state transition is the load-bearing safety
  step and is asserted — it is always legal from any QP state and fails only on a fatal
  HCA/programming fault, so a failure fail-stops the process rather than acking.
  `DisconnectAck` is therefore an *unconditional* guarantee that late writes can no
  longer land; there is no recoverable "error-and-withhold-ack" path. Freeing the queue
  pair (`rdma_destroy_qp`) is best-effort cleanup after the ERROR transition; its
  failure is logged, not fatal.
- Q: Does v1 include a feature-gated telemetry collector, or is `ILogger` enough? →
  A: Mirror the initiator (`remote-lookup-rdma-initiator`): add a `telemetry` feature
  that is a zero-sized no-op when disabled, plus a Criterion benchmark, for
  cross-component consistency.
- Q: How does `initialize()` choose the local RoCE IPv4 to bind and advertise, given
  it takes no address argument? → A: The mainline MAY supply it via a pre-`initialize()`
  Admin setter (`set_bind_ip(ip)`, parallel to `set_actor_cpu`); when it does not, the
  responder auto-detects the first RDMA device with an active port and binds its IPv4.
  `initialize()` binds the effective IP with port 0 and advertises it via
  `local_endpoint()`. The mainline (which owns NUMA placement) SHOULD supply an explicit
  IP on hosts with multiple RoCE NICs / NUMA nodes to keep the choice deterministic;
  either way the device is selected by IP, never by name (FR-002). Binding fails with
  `Bind` if the effective IP is missing/unusable (including no active device found).
- Q: If the actor→`remote-lookup` event channel is full, what is the delivery policy
  (notably for the load-bearing `DisconnectAck`)? → A: Backpressure — the actor
  blocks/retries until the event is enqueued; events are **never dropped**. Because
  `remote-lookup` is precisely the party blocked awaiting `DisconnectAck`, it is
  guaranteed to drain the channel, so lossless delivery cannot deadlock on the
  safety-critical path. A full channel is never treated as an error or a fail-stop.
- Q: How is SC-003's "prompt" command servicing made measurable/testable? → A: By a
  **structural** assertion, not a numeric wall-clock bound: over the mock CM seam,
  inject a `Disconnect` with zero pending connections and assert the `DisconnectAck` is
  delivered on an event-driven wake — with no intervening connection event and without
  waiting a poll cycle. This is deterministic and robust to CI jitter, and directly
  proves the loop is not blocked behind accept (FR-004).

## Boundary with `remote-lookup` (out of scope here)

- **Value lookup, the whisper status vector, and slot reclamation** live in
  `remote-lookup`. The one-sided completion signal travels back over the zyre
  whisper reply owned by `remote-lookup`, not over this interface. This component
  carries **control traffic only** on its channel — there is no data path and no
  per-value command.
- **The responder is the memory-tier registrar.** It gains a `memory_tier`
  receptacle, reads the whole DRAM pool via `IMemoryTier::pool_info()` at
  `initialize()`, and registers it **once** with `ibv_reg_mr` (`REMOTE_WRITE`) in
  its own protection domain — so inbound one-sided writes are bounds-checked
  against that PD. There is **no per-request `ibv_reg_mr`**; the single pool-wide
  region (base, `rkey`, length) is exposed via `local_region()` for `remote-lookup`
  to advertise. The memory tier itself stays RDMA-agnostic (it owns no MR). The
  responder still never reads or copies the value bytes.
- **`PeerId` ownership stays with `remote-lookup`.** It knows peers from zyre and
  passes `PeerId`s across the control channel; the `PeerId → queue pair` mapping
  lives *inside* the responder, populated from `private_data`. `remote-lookup` never
  holds a queue pair or connection handle.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Bind and advertise a listening endpoint (Priority: P1)

The application mainline initializes the responder. It first supplies the local
RoCE IPv4 via `set_bind_ip()`, then `initialize()` binds an ephemeral port on that
IP, starts the `rdma_cm` accept loop on a dedicated thread, and reads back the
assigned port. `remote-lookup` then calls `local_endpoint()` to get the `{ip, port}`
it advertises to serving peers in its whispers.

**Why this priority**: Nothing else can happen until the responder is listening and
its endpoint is discoverable — this is the MVP slice that makes the requesting node
addressable.

**Independent Test**: Initialize the component and assert `local_endpoint()`
returns a bound `{ip, port}` with a non-placeholder port; assert it errors with
`NotInitialized` before `initialize()`.

**Acceptance Scenarios**:

1. **Given** an uninitialized responder, **When** `local_endpoint()` is called,
   **Then** it returns `NotInitialized`.
2. **Given** `initialize()` has succeeded, **When** `local_endpoint()` is called,
   **Then** it returns the bound `{ip, port}` with the OS-assigned ephemeral port.
3. **Given** an already-initialized responder, **When** `initialize()` is called
   again, **Then** it returns `AlreadyInitialized` and the running loop is
   undisturbed.

---

### User Story 2 - Accept connections and correlate identity (Priority: P1)

A serving initiator connects in over `rdma_cm`. The accept loop reads the zyre UUID
the initiator stamped into the connect `private_data`, keys its per-node connection
table by that `PeerId`, and accepts the connection. It emits
`ConnectionEstablished { node }` on the control channel. A connection arriving with
absent or malformed `private_data` is still accepted but surfaces as
`ConnectionEstablished { node: None }`.

**Why this priority**: Without identity correlation there is no way to later address
a specific peer's queue pair for teardown, which is the memory-safety linchpin.

**Independent Test**: Drive a `CONNECT_REQUEST` (real loopback or a mock CM seam)
carrying a known UUID and assert `ConnectionEstablished { node: Some(peer) }` with
that `PeerId`; drive one with empty `private_data` and assert `node: None`.

**Acceptance Scenarios**:

1. **Given** an inbound connect stamped with a valid zyre UUID, **When** the accept
   loop handles it, **Then** it records a `PeerId`-keyed connection in `Active` state
   and emits `ConnectionEstablished { node: Some(peer) }`.
2. **Given** an inbound connect with absent or malformed `private_data`, **When** the
   accept loop handles it, **Then** it emits `ConnectionEstablished { node: None }`
   and the connection is reclaimable only via `shutdown`.
3. **Given** a peer that already has an `Active` connection, **When** a second
   connect arrives for the same `PeerId`, **Then** the responder handles it without
   corrupting the existing entry's state machine.

---

### User Story 3 - Teardown before reclaim (Priority: P1)

On a peer's departure (zyre EXIT), `remote-lookup` sends `Disconnect { node }` over
the control channel. The responder transitions that peer's connection
`Active → Draining`, transitions its RC queue pair into the ERROR state so late
one-sided writes are NAKed, transitions it to `Dead`, and only then replies
`DisconnectAck { node }`. `remote-lookup` blocks on that ack before reclaiming the
peer's locked landing slots.

**Why this priority**: This ordering is the sole guard against a late one-sided write
landing in memory that has already been reclaimed — a use-after-free / memory
corruption. It is load-bearing.

**Independent Test**: Establish a connection (loopback or mock), send
`Disconnect { node }`, and assert exactly one `DisconnectAck { node }` is returned
only after the queue pair is torn down; assert `Disconnect` for an unknown `PeerId`
is acknowledged idempotently.

**Acceptance Scenarios**:

1. **Given** an `Active` connection to `node`, **When** `Disconnect { node }` is
   received, **Then** the queue pair is transitioned to ERROR before
   `DisconnectAck { node }` is emitted, and the connection ends in `Dead`.
2. **Given** no connection (or an already-`Dead` one) for `node`, **When**
   `Disconnect { node }` is received, **Then** it is acknowledged with
   `DisconnectAck { node }` as a no-op (idempotent).
3. **Given** a connection in `Draining`, **When** a new fill/connect for the same
   `node` would arrive, **Then** it is refused — teardown is not raced by new work.

---

### User Story 4 - Prompt command servicing (Priority: P2)

The accept loop must service `Disconnect` commands promptly even while blocked
waiting for connections. It waits on `{rdma_cm channel fd, command inbox, stop
signal}` together, so a teardown request is never stuck behind a blocking accept.

**Why this priority**: Teardown-before-reclaim (Story 3) is only *safe* if it is also
*prompt*; a teardown that waits seconds behind an idle accept stalls slot reclamation
on the requester.

**Independent Test**: With no inbound connections pending, send `Disconnect` and
assert the `DisconnectAck` returns within a small bound rather than after the next
connection or a poll timeout.

**Acceptance Scenarios**:

1. **Given** an idle accept loop with no pending connections, **When** a command is
   enqueued, **Then** the loop wakes and services it without waiting for a connection.
2. **Given** the stop signal is raised, **When** the loop is waiting, **Then** it
   wakes and exits promptly.

---

### User Story 5 - Lifecycle and NUMA placement (Priority: P2)

The application mainline pins the accept-loop thread to the instance's NUMA node
before starting it, can signal the loop to stop without joining, and can shut it down
— joining the thread and tearing down all remaining connections and the listener.

**Why this priority**: Correct placement and clean teardown matter for a production
deployment but are not required to demonstrate a single accept/teardown cycle.

**Independent Test**: Call `set_actor_cpu` then `initialize`, exercise a command, then
`shutdown` and assert the thread joins and repeat `shutdown` is a no-op; assert
`signal_stop` causes the loop to exit.

**Acceptance Scenarios**:

1. **Given** `set_actor_cpu(n)` before `initialize()`, **When** the loop starts,
   **Then** its thread runs on NUMA node `n`.
2. **Given** a running responder, **When** `shutdown()` is called, **Then** the
   accept-loop thread is joined, all connections are torn down, and a second
   `shutdown()` is a no-op.
3. **Given** a running responder, **When** `signal_stop()` is called, **Then** the
   accept loop exits cooperatively without the thread being joined.

---

### User Story 6 - Operator telemetry (Priority: P3)

With the `telemetry` feature enabled, the responder records connection and teardown
metrics for diagnosis and capacity planning; with it disabled the collector is a
zero-sized no-op. This mirrors the initiator (`remote-lookup-rdma-initiator`) for
cross-component consistency.

**Why this priority**: Valuable in production but not required for functional
correctness — the responder works without it.

**Independent Test**: Build with `--features telemetry`, drive accept/disconnect
cycles over the loopback/mock CM seam, and read the recorded metrics.

**Acceptance Scenarios**:

1. **Given** the feature enabled, **When** connections are accepted and torn down,
   **Then** inbound connections accepted, identified vs unidentified (`node: None`),
   teardowns (disconnect-acks), and accept-loop errors are recorded.
2. **Given** the feature disabled, **When** the responder runs, **Then** call sites
   incur no cost (the collector is a ZST no-op).

---

### Edge Cases

- `local_endpoint()` or `open_control_channel()` called before `initialize()`
  (→ `NotInitialized`).
- `initialize()` called twice (→ `AlreadyInitialized`; running loop undisturbed).
- `open_control_channel()` called twice (→ `ChannelClosed`; single-client channel).
- Inbound connect with absent/garbage `private_data` (→ `ConnectionEstablished
  { node: None }`; not addressable by `disconnect`, reclaimed only on `shutdown`).
- `Disconnect` for a `PeerId` with no live connection (→ idempotent `DisconnectAck`).
- A second connect for a peer already `Active` (existing state machine not corrupted).
- `initialize()` called without a prior `set_bind_ip()` (→ auto-detect the first active
  device; `Bind` only if none is found).
- Bind fails because the effective RoCE IPv4 (supplied or auto-detected) is
  unavailable/unusable (→ `Bind`).
- Event channel full when emitting an event (e.g. a `DisconnectAck` while `remote-lookup`
  is momentarily behind) → the actor backpressures until it drains; the event is never
  dropped and this is not an error.
- QP→ERROR transition fails during teardown (implies a fatal HCA or programming
  fault) → the process fail-stops; `DisconnectAck` is never sent on a broken
  safety guarantee. `rdma_destroy_qp` cleanup failure afterward is logged, not fatal.
- Wedged-but-alive peer: R stops replying but its zyre heartbeat persists — slots
  stay locked until zyre expires R (bounded, tens of seconds) or the local stall
  escalates to a node-level teardown. The responder never frees on a per-operation
  timeout.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The component MUST be an actor — it MUST own a dedicated thread running
  an `rdma_cm` accept loop, separate from any caller thread.
- **FR-002**: `initialize()` MUST bind an ephemeral port (port 0) on the effective
  RoCE IPv4 (see FR-002a), start the accept loop (`rdma_listen`), and read back the
  assigned port via `rdma_get_src_port`. It MUST NOT pin the RDMA device by name;
  binding by IP implies the NIC/NUMA path. If the effective IP is missing/unusable on
  this host (including when auto-detection finds no active device), `initialize()` MUST
  return `Bind`. Calling it twice MUST return `AlreadyInitialized`.
- **FR-002a**: The bind IPv4 is resolved with this precedence: (1) an explicit address
  supplied by the mainline via the pre-`initialize()` Admin setter `set_bind_ip(ip)`
  (parallel to `set_actor_cpu`); otherwise (2) **auto-detect the first RDMA device with
  an active port** and bind its IPv4 (read from the port's RoCE v2 IPv4-mapped GID). The
  mainline (which owns NUMA placement) SHOULD supply an explicit IP on hosts with
  multiple RoCE NICs / NUMA domains to keep the choice deterministic; the
  `certus-server-yaml` mainline sources it from the `CERTUS_RDMA_BIND_IP` environment
  variable and passes it through, falling back to auto-detect when unset. The device is
  never pinned by name.
- **FR-003**: `local_endpoint()` MUST return the bound `{ip, port}` — the effective
  bind IP (supplied or auto-detected) and the OS-assigned ephemeral port — after initialization and
  `NotInitialized` before it, so `remote-lookup` can advertise the endpoint in whispers.
- **FR-004**: The accept loop MUST wait on `{rdma_cm channel fd, command inbox, stop
  signal}` together (e.g. `epoll`) so that `Disconnect` commands and stop are serviced
  promptly and never block behind a pending accept.
- **FR-005**: On `CONNECT_REQUEST` the responder MUST read the zyre UUID from the
  connect `private_data`, key its per-node connection table by that `PeerId`, accept
  the connection, and emit `ConnectionEstablished { node: Some(peer) }`.
- **FR-006**: A connect whose `private_data` is absent or malformed MUST still be
  accepted but MUST surface as `ConnectionEstablished { node: None }`; such a
  connection is reclaimable only via `shutdown`.
- **FR-007**: Each per-node connection MUST be guarded by an
  `Active → Draining → Dead` state machine. New fills/connects MUST be refused while a
  node is `Draining`.
- **FR-008**: On `Disconnect { node }` the responder MUST transition the RC queue pair
  to `node` into the ERROR state **before** emitting `DisconnectAck { node }`, so that
  late one-sided writes are NAKed and cannot land after slot reclamation. The
  ERROR-state transition is the load-bearing safety step and MUST be asserted: it is
  always legal from any QP state and fails only on a fatal HCA/programming fault, so a
  failure MUST fail-stop the process rather than emit an ack. `DisconnectAck` is
  therefore an unconditional guarantee that late writes can no longer land — there is
  no recoverable "error-and-withhold-ack" path. Freeing the queue pair
  (`rdma_destroy_qp`) is best-effort cleanup performed after the ERROR transition; its
  failure MUST be logged, not fatal. `Disconnect` for an unknown or already-dead `node`
  MUST be acknowledged idempotently.
- **FR-009**: The responder MUST NOT read, copy, or otherwise touch value bytes — it
  manages connections only. The data lands via the peer's one-sided RDMA write into
  the pool the responder registered.
- **FR-010**: At `initialize()` the responder MUST read the whole memory-tier pool via
  the `memory_tier` receptacle's `IMemoryTier::pool_info()` and register it **once**
  with `ibv_reg_mr` (`REMOTE_WRITE`) in the listener's protection domain (there is no
  per-request `ibv_reg_mr`). It MUST expose the resulting pool-wide region — base
  address, `rkey`, and length — via `local_region()`, and MUST deregister the region
  (`ibv_dereg_mr`) before its PD is freed on shutdown. If the `memory_tier` receptacle
  is unbound, the pool is uninitialized, or `ibv_reg_mr` fails, `initialize()` MUST
  return `Registration`.
- **FR-011**: `open_control_channel()` MUST hand `remote-lookup` a single-client
  control channel (command sender + event receiver); a second call MUST return
  `ChannelClosed`.
- **FR-011a**: Event delivery to `remote-lookup` MUST be lossless: if the event channel
  is full, the actor MUST apply backpressure (block/retry until the event is enqueued)
  rather than drop the event, fail-stop, or turn it into an error. This is load-bearing
  for `DisconnectAck` — dropping it would stall slot reclamation forever — and is
  deadlock-free because `remote-lookup` is the party blocked awaiting that ack and so
  drains the channel.
- **FR-012**: `set_actor_cpu(cpu)` MUST be honored before `initialize()` to pin the
  accept-loop thread to the instance's NUMA node.
- **FR-013**: `signal_stop()` MUST cause the accept loop to exit cooperatively without
  joining; `shutdown()` MUST stop and join the accept-loop thread and tear down all
  remaining connections and the listener, and MUST be idempotent.
- **FR-014**: Diagnostics MUST route through an optional `ILogger` receptacle; a
  missing logger MUST NOT turn any operation into an error.
- **FR-015**: The component MUST include unit tests covering the lifecycle
  (initialize / open channel / disconnect-ack / shutdown), the `NotInitialized` /
  `AlreadyInitialized` / `ChannelClosed` error paths, and the connection state machine.
- **FR-016**: The component MUST optionally collect telemetry behind a `telemetry`
  feature, zero-cost (a ZST no-op) when disabled, mirroring
  `remote-lookup-rdma-initiator`. Metric set: inbound connections accepted, identified
  vs unidentified (`node: None`), teardowns (disconnect-acks emitted), and accept-loop
  errors.

### Key Entities

- **Endpoint**: `{ ip: String, port: u16 }` — the bound listening endpoint; `ip` is
  the *effective* bind address resolved per FR-002a's precedence — an explicit
  address supplied by the mainline via `set_bind_ip()` before `initialize()`,
  else auto-detected from the first RDMA device with an active port — and
  `port` is ephemeral (assigned at bind, read back via `rdma_get_src_port`).
  Advertised by `remote-lookup` in whispers.
- **PeerId**: a zyre node identity (UUID). Owned by `remote-lookup`; used by the
  responder as the connection-table key, resolved from connect `private_data`.
- **ResponderCommand**: control command *to* the actor — `Disconnect { node }`. No
  per-value command (writes are one-sided).
- **ResponderEvent**: event *from* the actor — `ConnectionEstablished { node:
  Option<PeerId> }`, `DisconnectAck { node }`, `Error { message }`.
- **ControlChannel**: single-client `{ command_tx, event_rx }` pair handed to
  `remote-lookup` by `open_control_channel()`. Event delivery on `event_rx` is lossless
  — the actor backpressures rather than drop when the channel is full (FR-011a).
- **Per-node connection / state machine**: `Active → Draining → Dead`, keyed by
  `PeerId`, guarding teardown against concurrent fills. Internal to the responder.
- **RemoteLookupRdmaResponderError**: method-level failure — `NotInitialized`,
  `AlreadyInitialized`, `Bind`, `ChannelClosed`, `Internal`. Per-connection outcomes
  are reported via `ResponderEvent`, not here.
- **Telemetry collector** (feature-gated): a ZST no-op when the `telemetry` feature is
  off; when on, counts connections accepted, identified vs unidentified, teardowns, and
  accept-loop errors. Mirrors the initiator's collector.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: After `initialize()`, `local_endpoint()` returns a bound endpoint whose
  port is the OS-assigned ephemeral port (non-placeholder), and returns
  `NotInitialized` beforehand — validated by unit tests.
- **SC-002**: A `Disconnect { node }` command is answered with exactly one
  `DisconnectAck { node }`, and the queue-pair ERROR-state transition is observably
  ordered *before* the ack — validated against the loopback/mock CM seam.
- **SC-003**: An idle accept loop (no pending connections) services an enqueued
  command and returns its ack **without any intervening connection event and without
  waiting a poll cycle** — i.e. the wait is event-driven, not timeout-driven. Validated
  structurally over the mock CM seam (inject a command with zero pending connects and
  assert the ack is delivered on an event-driven wake, not after a sleep/poll interval),
  which is deterministic and jitter-proof; no numeric wall-clock bound is asserted.
- **SC-004**: Two co-resident responder instances sharing one NIC bind distinct
  ephemeral ports and each advertises its own `{ip, port}` without collision.
- **SC-005**: A connect stamped with a known zyre UUID yields
  `ConnectionEstablished { node: Some(peer) }` carrying that `PeerId`; a connect with
  empty `private_data` yields `node: None`.
- **SC-006**: When the `telemetry` feature is enabled, connection/teardown metrics are
  available with less than 5% performance overhead versus the disabled build, measured
  by a Criterion benchmark that drives the accept/disconnect path over the mock CM seam
  and compares the feature-on build against a feature-off baseline (mirroring the
  initiator's two-run workflow).

## Assumptions

- The responder runs on RDMA-capable hardware on an isolated, trusted fabric; security
  is network-level trust only (consistent with the initiator spec).
- The memory tier is allocated once at startup and does not grow, so the responder can
  register the whole pool once (with a stable `rkey`) at `initialize()` before any peer
  writes into it; growing the pool would need a re-registration protocol (out of scope).
- All serving initiators stamp their own zyre UUID into the connect `private_data`;
  this is a protocol invariant. Unstamped connections are tolerated but only
  reclaimable via `shutdown`.
- `remote-lookup` owns zyre, the whisper control plane, slot locking/reclamation, and
  issues `Disconnect` on zyre EXIT (never on a per-operation timeout while the peer is
  still a live member).
- One Certus instance runs per NUMA domain; co-resident instances may share a single
  RDMA NIC, which is why the listen port is ephemeral and the device is selected by IP.
- Telemetry is opt-in (feature-gated) and disabled by default, matching the initiator.
- The QP→ERROR transition is always legal from any QP state and fails only on a fatal
  HCA or programming fault, so asserting it (fail-stop on failure) is safe and keeps
  `DisconnectAck` an unconditional guarantee.

## Build & Feature Flags

- **`rdma` Cargo feature** gates the entire production `rdma_cm`/`ibv_reg_mr`
  implementation (real `bind`/`rdma_listen`/`rdma_get_src_port`, `epoll` over the
  CM fd, `private_data` read, whole-pool `ibv_reg_mr`, real QP teardown). It is
  off by default: without it the crate builds and unit-tests entirely over the
  in-process mock CM seam, with no `rdma-core` (`libibverbs`/`librdmacm`)
  libraries present, and `initialize()` is unavailable — it returns `Bind` with a
  message stating the crate was built without the `rdma` feature. This is a
  **build-configuration** failure mode, distinct from the FR-002/FR-010 runtime
  `Bind`/`Registration` failures (missing/unusable IP, no active device,
  `ibv_reg_mr` failure) that occur when the feature *is* enabled. Mainline apps
  that wire the responder to real hardware enable `rdma`; CI and the workspace
  default-members build do not (mirrors `remote-lookup-rdma-initiator` and
  `block-device-spdk-nvme`'s SPDK-gating pattern).
- **`telemetry` Cargo feature** — see FR-016 / User Story 6.

## Known Limitations / Follow-ups

- **Skeleton first, hardware loop later.** The initial implementation lands the actor
  scaffolding, control channel, lifecycle, and state machine over a mock/loopback CM
  seam; the production `rdma_cm` accept loop (`bind` + `rdma_listen` +
  `rdma_get_src_port`, `epoll` over the CM fd, `private_data` read, real QP teardown,
  NUMA pinning) is verified on RDMA hardware in a follow-up.
- **Unstamped connections** are not addressable by `disconnect(PeerId)` and are
  reclaimed only on `shutdown` (or a future wedged-node backstop). The mandatory-
  stamping invariant keeps this a should-never-happen path.
- **Wedged-but-alive peer.** If a peer stops replying but its zyre heartbeat persists,
  slots stay locked until zyre expiry (bounded, tens of seconds) or a node-level
  teardown; the responder never frees on a per-operation timeout.
- **Initiator UUID stamping** is a cross-component requirement: the initiator must
  know its own local `PeerId` (supplied at init) and stamp it on every connect. Tracked
  against the initiator, not here.
- **SC-006 telemetry overhead** is covered by a Criterion benchmark (`benches/`); run
  the two-baseline (feature on/off) workflow to confirm the < 5% budget on target
  hardware, mirroring the initiator's `push_telemetry` benchmark.
