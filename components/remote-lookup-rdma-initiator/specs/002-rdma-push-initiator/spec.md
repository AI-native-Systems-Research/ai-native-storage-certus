# Feature Specification: RDMA Push Initiator

**Feature Branch**: `rework-remote-lookup-rdma-initiator`

**Created**: 2026-07-09

**Status**: Draft

**Supersedes**: `001-rdma-remote-lookup-rdma-initiator` (RDMA Remote Lookup Initiator)

**Input**: The data-holding (server) side of a remote lookup. Driven by the
`remote-lookup` component: given a peer's host endpoint and a batch of
`(key, remote-region)` pairs, connect out, resolve each key in the local memory
tier, and RDMA-write matching values directly into the peer's memory.

## Clarifications

### Carried forward from spec-001

- Keys are a 64-bit `CacheKey` (identifies the cached object) plus a 32-bit RDMA
  memory key (`rkey`) carried per remote region.
- Security is network-level trust only (an isolated RDMA fabric provides the
  security perimeter); no application-level authentication is required.

### Session 2026-07-09 (initiator model)

- Q: Which direction is the RDMA data path? → A: Outbound. This component is the
  initiator; it connects out and RDMA-*writes* local values into a remote peer's
  memory. There is no inbound listener here.
- Q: Where do lookups resolve? → A: Against the local memory tier via
  `IMemoryTier::peek` — not a separate dispatch service.
- Q: How are connection failures handled? → A: Detect a queue pair in the error
  state (or a failed in-flight write), tear down and rebuild the connection once,
  and retry the batch.

## Boundary with `remote-lookup` (out of scope here)

- The RDMA **accept** side (running an `rdma_cm` listener, pre-registering receive
  buffers with remote-write access) and the **zyre control plane** (carrying keys
  and `RemoteRegion` descriptors between peers) live in the `remote-lookup`
  component.
- This component owns only the **outbound RDMA data path** and is invoked via the
  `IRemoteLookupRdmaInitiator` interface.
- This component's half of the responder's "teardown-before-reclaim" contract is
  peer identification: `set_local_peer_id` (FR-015) stamps this node's zyre
  `PeerId` into every outbound connect so the `remote-lookup-rdma-responder` on
  the far end can correlate an inbound queue pair to the connecting peer before
  reclaiming it. Reading and acting on that correlation is the responder's
  responsibility, not this component's.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Push cached values into a remote host (Priority: P1)

`remote-lookup` calls `push(endpoint, items)`, where each item is a
`(CacheKey, RemoteRegion { addr, rkey, length })`. The handler resolves each key
against the local memory tier and, when the value is present with a matching size,
RDMA-writes it directly into the remote region. It returns one `PushStatus` per
item, in input order.

**Why this priority**: This is the core value proposition — satisfying a remote
peer's lookup by placing data directly into its memory with no intermediate copy.

**Independent Test**: Drive `push` with a bound memory tier (real or mock) and a
mock RDMA transport; verify per-item statuses and that writes target the specified
regions.

**Acceptance Scenarios**:

1. **Given** a bound memory tier with an initialized pool, **When** `push` is
   called with items whose keys are present and whose value sizes match the region
   lengths, **Then** each matching value is RDMA-written into its remote region and
   its item reports `Success`.
2. **Given** a key absent from the local memory tier, **When** `push` processes
   that item, **Then** it reports `KeyNotFound` and no write is attempted, without
   affecting other items.
3. **Given** a key present but whose value size differs from `region.length`,
   **When** `push` processes that item, **Then** it reports `SizeMismatch` (no
   partial write).
4. **Given** no connection to the host can be established, **When** `push` is
   called, **Then** every item reports `UnableToConnect`.
5. **Given** the `memory_tier` receptacle is unbound or its pool is uninitialized,
   **When** `push` is called, **Then** it returns `NotInitialized`.
6. **Given** an endpoint that is not a valid `"ip:port"`, **When** `push` is called
   (with a bound memory tier), **Then** it returns `InvalidEndpoint`.

---

### User Story 2 - Connection reuse and self-repair (Priority: P1)

Establishing an RDMA/RoCE CM connection was measured at more than two seconds, so
connections are established lazily on first push to a host and reused across calls,
held in a table keyed by the normalized `"ip:port"` endpoint. A caller may also
**warm** a connection ahead of time with `connect(endpoint)` — establishing it
without writing — so the multi-second cold connect happens off the hot path (e.g.
at peer discovery) and the first real `push` is fast.

**Why this priority**: Without reuse, every push would pay the multi-second connect
cost; without self-repair, a single transient QP fault would strand a host; without
warm-connect, the first push to each peer pays the cold-connect latency inline.

**Independent Test**: With a mock transport, push twice to one host and assert only
one connect occurs; `connect` then `push` to the same host and assert the push adds
no second connect; force the mock's QP into the error state and assert exactly one
reconnect-and-retry.

**Acceptance Scenarios**:

1. **Given** no existing connection to a host, **When** `push` targets it, **Then**
   a connection is established lazily and cached for reuse.
2. **Given** concurrent pushes to *different* hosts, **When** they run, **Then**
   they proceed concurrently; **Given** concurrent pushes to the *same* host,
   **Then** they serialize on that host's slot (a queue pair is not safe for
   concurrent use).
3. **Given** a cached connection whose queue pair is in the error state, or an
   in-flight write that fails, **When** `push` runs, **Then** the connection is
   torn down and rebuilt **once** and the batch retried; a second failure yields
   `UnableToConnect` for the affected items.
4. **Given** no connection to a host, **When** `connect(endpoint)` is called
   (warm-connect), **Then** a connection is established and cached so a later
   `push` reuses it (no new CM connect); **Given** a healthy cached connection,
   **When** `connect` is called again, **Then** it is a no-op; **Given** the host
   is unreachable, **When** `connect` is called, **Then** it returns `Ok(())`
   with nothing cached (a transient failure is not surfaced as an error) and the
   next `connect`/`push` retries.

---

### User Story 3 - Teardown (Priority: P2)

A caller tears down connections when a host is known to have left the cluster.

**Why this priority**: Bounded resource use and clean shutdown; not required for a
single push to succeed.

**Independent Test**: Connect via the mock transport, call `disconnect` /
`disconnect_all`, and verify the slot(s) are removed and repeat calls are no-ops.

**Acceptance Scenarios**:

1. **Given** a connected host, **When** `disconnect(endpoint)` is called, **Then**
   that host's connection is torn down; calling it for an unknown endpoint is a
   no-op (idempotent).
2. **Given** any set of connections, **When** `disconnect_all()` is called, **Then**
   all connections in the table are torn down.

---

### User Story 4 - Operator telemetry (Priority: P3)

With the `telemetry` feature enabled, the handler records connection and transfer
metrics for diagnosis and capacity planning; with it disabled the collector is a
zero-sized no-op.

**Why this priority**: Valuable in production but not required for functional
correctness — the handler works without it.

**Independent Test**: Build with `--features telemetry`, run pushes through the
mock transport, and read metrics via `RemoteLookupRdmaInitiatorComponent::telemetry()`.

**Acceptance Scenarios**:

1. **Given** the feature enabled, **When** pushes run, **Then** connections
   established/failed, reconnects, disconnects, push batches and average push
   duration, per-item outcomes (mirroring `PushStatus`), total bytes
   RDMA-written, and the per-phase connect-latency breakdown (address/route/
   connect/MR-registration µs, with a running average) are recorded.
2. **Given** the feature disabled, **When** pushes run, **Then** call sites incur no
   cost (the collector is a ZST no-op).

---

### Edge Cases

- The memory-tier pool is not yet initialized when `push` is first called
  (→ `NotInitialized`).
- An endpoint string lacks a port or is otherwise unparseable (→ `InvalidEndpoint`).
- A batch mixes present, absent, and size-mismatched keys (per-item statuses must be
  independent and ordered).
- A queue pair enters the error state mid-batch (→ one reconnect-and-retry).
- A value is evicted between `peek` and write completion (see Known Limitations —
  data-freshness concern, not memory safety).
- An empty `items` slice (→ empty status vector, no connection required beyond what
  the implementation already does).
- `push`/`connect` is called before `set_local_peer_id` — the resulting connection
  carries no peer identification in its `private_data` (responder sees `node: None`).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST expose
  `push(endpoint, items: &[(CacheKey, RemoteRegion)]) -> Result<Vec<PushStatus>, RemoteLookupRdmaInitiatorError>`,
  returning exactly one status per item in input order.
- **FR-002**: System MUST resolve each key against the local memory tier via
  `IMemoryTier::peek`, mapping absent keys to `KeyNotFound` and size mismatches
  (value size ≠ `region.length`) to `SizeMismatch` before attempting any write.
- **FR-003**: System MUST RDMA-write matching values directly into the
  caller-specified remote region using its `addr` and 32-bit `rkey`.
- **FR-004**: System MUST register the memory-tier pool (`IMemoryTier::pool_info`
  base + size) as an RDMA memory region once per connection; writes issue from the
  `peek` pointer, which lies within that registered region.
- **FR-005**: System MUST hold connections in a table keyed by the normalized
  `"ip:port"` endpoint, with per-host state
  (disconnected / connecting / connected / disconnecting), establishing lazily and
  reusing established connections across calls.
- **FR-006**: System MUST allow pushes to different hosts to proceed concurrently
  while serializing pushes to the same host on that host's slot (a queue pair is not
  safe for concurrent use).
- **FR-007**: System MUST detect a queue pair in the error state or a failed
  in-flight write, tear down and rebuild the connection **once**, and retry the
  batch; a second failure yields `UnableToConnect` for the affected items.
- **FR-008**: System MUST expose `disconnect(endpoint)` (idempotent) and
  `disconnect_all()` to tear down host-level connections.
- **FR-009**: System MUST parse endpoints as `"ip:port"` and return
  `InvalidEndpoint` for anything that does not parse.
- **FR-010**: System MUST route diagnostics through an optional `ILogger`
  receptacle, using a no-op logger when it is unbound so that a missing logger never
  turns a push into an error.
- **FR-011**: System MUST optionally collect telemetry behind the `telemetry`
  feature, zero-cost (a ZST no-op) when disabled. Metric set: outbound connections
  established/failed, reconnects, disconnects, push batches and average push
  duration, per-item outcomes, total bytes RDMA-written, and — for each successful
  connect — a per-phase latency breakdown in microseconds (rdma_cm address
  resolution, route resolution, connect handshake, and memory-region registration)
  with a running average, used to attribute cold-connect cost and retune the
  driving `remote-lookup` node's `op_deadline` / `phase1_timeout`.
- **FR-014**: System MUST expose `connect(endpoint)` — proactively establish
  (warm) a connection to `endpoint` without writing — so the driving node can move
  the multi-second cold connect off its hot path (e.g. at peer discovery) and a
  later `push` to that endpoint hits the established-connection fast path. It MUST
  be idempotent and connection-caching like `push` (a healthy existing connection
  is a no-op), and a connection that cannot be established MUST return `Ok(())`
  with nothing cached — the next `connect`/`push` retries — so warming never
  surfaces a transient network failure as an error. It MUST return `NotInitialized`
  when the memory-tier receptacle is unbound or its pool is uninitialized, and
  `InvalidEndpoint` for an unparseable `"ip:port"`.
- **FR-012**: Security relies on trusted-fabric network isolation; no
  application-level authentication is required.
- **FR-013**: System MUST include unit tests covering the connection-table state
  machine, `PushStatus` mapping, a mock RDMA transport seam, and telemetry wiring.
- **FR-015**: System MUST expose `set_local_peer_id(peer: PeerId)` to record this
  node's zyre `PeerId`. From that point on, every outbound `rdma_cm` connect
  established by `push`/`connect` MUST stamp `peer` into the connection's
  `private_data` so the remote `remote-lookup-rdma-responder` can correlate the
  inbound queue pair to the connecting peer — required for the responder's
  teardown-before-reclaim flow. It SHOULD be called once, before the first
  `push`; connections established before it is set carry no peer identification
  (the responder sees `node: None` for them, reclaimable only via its backstop
  shutdown).

### Key Entities

- **RemoteRegion**: `{ addr: u64, rkey: u32, length: u32 }` — a descriptor of the
  remote destination memory supplied by the requesting node.
- **PushStatus**: per-item outcome — `Success | UnableToConnect | KeyNotFound |
  SizeMismatch`.
- **CacheKey**: 64-bit identifier resolved against the local memory tier.
- **ConnectionTable / HostSlot / ConnState**: per-host outbound connection state,
  keyed by `"ip:port"`.
- **RdmaTransport / RdmaConn / RealTransport**: the testable transport seam — a real
  `rdma_cm`-backed transport in production, a mock in tests.
- **RemoteLookupRdmaInitiatorError**: method-level failure — `NotInitialized` or
  `InvalidEndpoint`. Per-item conditions are reported via `PushStatus`, not here.
- **PeerId**: This node's zyre discovery-layer identifier, supplied once via
  `set_local_peer_id` and stamped into the `rdma_cm` connect `private_data` of
  every subsequent outbound connection so the responder can correlate an inbound
  queue pair back to the connecting peer (teardown-before-reclaim).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: `push` returns exactly one `PushStatus` per input item, in order, with
  correct terminal statuses for absent keys and size mismatches (validated by unit
  tests against the mock transport).
- **SC-002**: A second push to an already-connected host reuses the connection (no
  new CM connect), avoiding the measured >2 s establishment cost.
- **SC-003**: A queue-pair error triggers exactly one reconnect-and-retry before an
  item is reported as `UnableToConnect`.
- **SC-004**: When the `telemetry` feature is enabled, its per-push cost MUST be a
  small fixed constant — a handful of `Relaxed` atomic counter updates (one push +
  duration accumulate, plus one per item) — and a zero-sized no-op when disabled,
  so it is negligible relative to a real RDMA write (µs–ms). Measured by the
  `push_telemetry` Criterion benchmark (`benches/`) over a mock transport (two-run
  on/off workflow, README "Benchmark"). **Measured 2026-07-15 (mock transport):
  on ≈ +13 ns/push (push/1 211 ns vs 195 ns) rising with item count (push/16
  +13%, push/64 +8%).** NOTE: the mock push is a ~200–700 ns no-op, so a few
  unavoidable atomics read as 6–13% *of the mock* — this **exceeds a naive <5%
  microbenchmark budget by construction** and does not reflect production, where a
  push performs an actual one-sided RDMA write that dwarfs the counters (<0.1%).
  The criterion is therefore "small fixed absolute cost / ZST-when-off", not a
  percentage against the mock. (Superseded the original "< 5% vs disabled build"
  wording, which measured against an unrepresentative near-zero baseline.)

## Assumptions

- The handler runs on RDMA-capable hardware on an isolated, trusted fabric.
- The memory-tier pool is initialized before the first push, so its base/size are
  known and can be registered as an RDMA memory region.
- The `remote-lookup` component provides the accept side, receive-buffer
  registration (remote-write access), and the zyre control plane that delivers keys
  and `RemoteRegion` descriptors.
- Telemetry is opt-in (feature-gated) and disabled by default.

## Known Limitations / Follow-ups

- **Accept side lives in `remote-lookup`.** For `push`'s `rdma_connect` to succeed,
  the requesting node must run an `rdma_cm` listener and pre-register its receive
  memory with remote-write access, then communicate the endpoint and
  `RemoteRegion`s. That is the `remote-lookup` component's responsibility.
- **Eviction race.** `peek` returns a pointer/size without pinning the entry against
  eviction; an eviction + reallocation between `peek` and write completion could
  change the bytes (the pointer stays within the registered pool, so this is a
  data-freshness concern, not memory safety). Pinning (a dispatch-map read reference
  or a memory-tier pin API) will be added when integrating with `remote-lookup`.
- **SC-004 telemetry overhead** is covered by the `push_telemetry` benchmark
  (`benches/push_telemetry.rs`); run the two-baseline workflow (README) to confirm
  the < 5% budget on target hardware.
