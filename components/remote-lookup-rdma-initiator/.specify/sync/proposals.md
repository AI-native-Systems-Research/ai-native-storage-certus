# Drift Resolution Proposals

Generated: 2026-07-09 (interactive)
Based on: drift-report 2026-07-09T18:24:27Z
Strategy: **Retire spec-001, draft spec-002** (user-selected)

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Retire / Supersede spec | 1 (spec-001) |
| New Spec | 1 (spec-002, backfilling 6 unspecced features) |
| Backfill (Code → Spec) | folded into spec-002 (was 8 drifted) |
| Remove from Spec | folded into retirement (was 11 not-implemented) |
| Align (Spec → Code) | 0 |
| Human Decision | 0 (strategy resolved interactively) |

**Rationale for strategy:** `spec-001` describes the original *passive RDMA responder*
(inbound listener → session state machine → protobuf handshake → RDMA-write results
back to the caller). The component was reworked into an *outbound initiator*
(`push(endpoint, items)` → look up locally in the memory tier → RDMA-write into the
requester's memory), and the entire responder stack (`listener`/`session`/`serve`/
`protocol` + both bins + `proto/`) was removed. With 48% of requirements
not-implemented and 35% inverted against a design that no longer exists,
per-requirement patching is not worthwhile. The 8 "drifted" items are the same
concepts inverted; the 11 "not-implemented" items were deleted (mostly relocated to
the `remote-lookup` component). Both sets are captured below as a single
retire-and-respecify move.

---

## Proposal 1: Retire spec-001

**Direction**: RETIRE (supersede, keep for history)

**Current State**:
- `specs/001-rdma-remote-lookup-rdma-initiator/spec.md` — Status: Draft, describes the
  passive responder that no longer exists in code.

**Proposed Resolution**:
- Set spec-001 front-matter **Status** to `Superseded by spec-002` and add a one-line
  banner at the top pointing to spec-002. Do **not** delete — it records the original
  design intent and the clarifications (batch cap, trusted-fabric security, key types)
  that partly carry forward.
- The following spec-001 requirements are **removed** by the rework (do not migrate):
  - FR-001 (accept inbound connections) — accept side moved to `remote-lookup`.
  - FR-002 (version handshake) — no wire protocol in this component.
  - FR-005 (reject > 64 batch) — no batch cap exists.
  - FR-006 (unsignaled-write pipelining) — new path issues signaled writes per item.
  - FR-012 (standalone test client) — `bin/test_client.rs` removed.
  - FR-016 (binary wire protocol) — control plane moved to `remote-lookup` over zyre.
  - SC-001 (64-entry session latency), SC-002 (≥100 inbound sessions),
    SC-003 (test-client run), SC-004 (CM-disconnect cleanup <1s),
    SC-006 (handshake version mismatch) — all obsolete under the initiator model.

**Rationale**: The design was replaced wholesale, not amended. Retiring preserves
history while making spec-002 the single source of truth.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

## Proposal 2: New spec-002 — RDMA Push Initiator

**Direction**: NEW_SPEC (backfills all 6 unspecced features + the 8 inverted requirements)

**Feature**: Outbound RDMA "push" of locally-cached values into a remote host's memory.
**Location**: `interfaces/src/iremote_lookup_rdma_initiator.rs`,
`components/remote-lookup-rdma-initiator/src/{lib,connection,rdma,ffi,telemetry}.rs`

**Draft Spec**:

> # Feature Specification: RDMA Push Initiator
>
> **Feature Branch**: `rework-remote-lookup-rdma-initiator`
> **Created**: 2026-07-09
> **Status**: Draft
> **Supersedes**: spec-001 (RDMA Remote Lookup Initiator)
>
> **Input**: The data-holding (server) side of a remote lookup. Driven by the
> `remote-lookup` component: given a peer's host endpoint and a batch of
> `(key, remote-region)` pairs, connect out, resolve each key in the local memory
> tier, and RDMA-write matching values directly into the peer's memory.
>
> ## Carried-forward clarifications (from spec-001)
> - Keys are a 64-bit `CacheKey` plus a 32-bit RDMA memory key (`rkey`) per region.
> - Security is network-level trust only (isolated RDMA fabric); no app-level auth.
>
> ## Boundary with `remote-lookup` (not this component)
> - The RDMA **accept** side (running an `rdma_cm` listener, pre-registering
>   receive buffers with remote-write access) and the **zyre control plane**
>   (carrying keys + `RemoteRegion` descriptors) live in `remote-lookup`.
> - This component owns only the **outbound data path** and is invoked via
>   `IRemoteLookupRdmaInitiator`.
>
> ## User Scenarios & Testing
>
> ### User Story 1 - Push cached values into a remote host (Priority: P1)
> `remote-lookup` calls `push(endpoint, items)` where each item is a
> `(CacheKey, RemoteRegion{addr, rkey, length})`. The handler resolves each key
> against the local memory tier and, when present with a matching size, RDMA-writes
> the value into the remote region. It returns one `PushStatus` per item, in order.
>
> **Acceptance Scenarios**:
> 1. **Given** a bound memory tier with an initialized pool, **When** `push` is
>    called with items whose keys are present and sizes match the region lengths,
>    **Then** each matching value is RDMA-written into its remote region and its
>    item reports `Success`.
> 2. **Given** a key absent from the local memory tier, **When** `push` processes
>    that item, **Then** it reports `KeyNotFound` and no write is attempted, without
>    affecting other items.
> 3. **Given** a key present but whose value size differs from `region.length`,
>    **When** `push` processes that item, **Then** it reports `SizeMismatch` (no
>    partial write).
> 4. **Given** no connection to the host can be established, **When** `push` is
>    called, **Then** every item reports `UnableToConnect`.
> 5. **Given** the `memory_tier` receptacle is unbound or its pool is uninitialized,
>    **When** `push` is called, **Then** it returns `NotInitialized`.
> 6. **Given** an endpoint that is not a valid `"ip:port"`, **When** `push` is
>    called (with a bound memory tier), **Then** it returns `InvalidEndpoint`.
>
> ### User Story 2 - Connection reuse and self-repair (Priority: P1)
> Establishing an RDMA/RoCE CM connection was measured at >2s, so connections are
> established lazily on first push to a host and reused across calls, held in a
> table keyed by normalized `"ip:port"`.
>
> **Acceptance Scenarios**:
> 1. **Given** no existing connection to a host, **When** `push` targets it,
>    **Then** a connection is established lazily and cached for reuse.
> 2. **Given** concurrent pushes to *different* hosts, **When** they run,
>    **Then** they proceed concurrently; **Given** concurrent pushes to the *same*
>    host, **Then** they serialize on that host's slot (a queue pair is not safe for
>    concurrent use).
> 3. **Given** a cached connection whose queue pair is in the error state, or an
>    in-flight write that fails, **When** `push` runs, **Then** the connection is
>    torn down and rebuilt **once** and the batch retried; a second failure yields
>    `UnableToConnect` for the affected items.
>
> ### User Story 3 - Teardown (Priority: P2)
> A caller tears down connections when a host leaves the cluster.
>
> **Acceptance Scenarios**:
> 1. **Given** a connected host, **When** `disconnect(endpoint)` is called,
>    **Then** that host's connection is torn down; calling it for an unknown
>    endpoint is a no-op (idempotent).
> 2. **Given** any set of connections, **When** `disconnect_all()` is called,
>    **Then** all connections in the table are torn down.
>
> ### User Story 4 - Operator telemetry (Priority: P3)
> With the `telemetry` feature enabled, the handler records connection and transfer
> metrics; with it disabled the collector is a zero-sized no-op.
>
> **Acceptance Scenarios**:
> 1. **Given** the feature enabled, **When** pushes run, **Then** connections
>    established/failed, reconnects, disconnects, push batches + average duration,
>    per-item outcomes, and total bytes written are recorded (readable via
>    `RemoteLookupRdmaInitiatorComponent::telemetry()`).
> 2. **Given** the feature disabled, **When** pushes run, **Then** call sites incur
>    no cost (ZST no-op).
>
> ## Requirements
>
> ### Functional Requirements
> - **FR-001**: MUST expose `push(endpoint, items: &[(CacheKey, RemoteRegion)]) ->
>   Result<Vec<PushStatus>, RemoteLookupRdmaInitiatorError>`, returning one status per
>   item in input order. *(was FR-004, direction inverted)*
> - **FR-002**: MUST resolve each key against the local memory tier via
>   `IMemoryTier::peek`, mapping absent keys to `KeyNotFound` and size mismatches
>   (value size ≠ `region.length`) to `SizeMismatch`. *(was FR-010: dispatch →
>   memory tier)*
> - **FR-003**: MUST RDMA-write matching values directly into the caller-specified
>   remote region using `addr` + 32-bit `rkey`. *(carries FR-007, aligned)*
> - **FR-004**: MUST register the memory-tier pool (`IMemoryTier::pool_info` base +
>   size) as an RDMA memory region once per connection; writes issue from the
>   `peek` pointer within that region. *(unspecced → specced)*
> - **FR-005**: MUST hold connections in a table keyed by normalized `"ip:port"`
>   with per-host state (disconnected/connecting/connected/disconnecting),
>   establishing lazily and reusing across calls. *(was FR-003, inverted)*
> - **FR-006**: MUST allow pushes to different hosts to proceed concurrently while
>   serializing pushes to the same host on that host's slot. *(unspecced → specced)*
> - **FR-007**: MUST detect a queue pair in the error state or a failed in-flight
>   write, tear down and rebuild the connection once, and retry the batch; a second
>   failure yields `UnableToConnect`. *(was FR-009, inverted: CM I/O → QP health)*
> - **FR-008**: MUST expose `disconnect(endpoint)` (idempotent) and
>   `disconnect_all()` to tear down host-level connections. *(was FR-008, inverted:
>   session → host connection)*
> - **FR-009**: MUST parse endpoints as `"ip:port"` and return `InvalidEndpoint`
>   otherwise. *(unspecced → specced)*
> - **FR-010**: MUST route diagnostics through an optional `ILogger` receptacle,
>   using a no-op logger when unbound so a missing logger never fails a push.
>   *(carries FR-011, aligned)*
> - **FR-011**: MUST optionally collect telemetry behind the `telemetry` feature,
>   zero-cost (ZST no-op) when disabled; metric set per DESIGN.md. *(carries FR-014,
>   which now exceeds old spec — telemetry is wired)*
> - **FR-012**: Security relies on trusted-fabric network isolation; no app-level
>   auth. *(carries FR-017, aligned)*
> - **FR-013**: MUST provide unit tests for the connection-table state machine,
>   `PushStatus` mapping, a mock RDMA transport seam, and telemetry wiring.
>   *(was FR-013, inverted: session/protocol/listener tests → connection/transport)*
>
> ### Key Entities
> - **RemoteRegion**: `{ addr: u64, rkey: u32, length: u32 }` — remote destination.
> - **PushStatus**: `Success | UnableToConnect | KeyNotFound | SizeMismatch`.
> - **CacheKey**: 64-bit identifier resolved against the local memory tier.
> - **ConnectionTable / HostSlot / ConnState**: per-host outbound connection state.
> - **RdmaTransport / RdmaConn / RealTransport**: testable transport seam.
>
> ## Success Criteria
> - **SC-001**: `push` returns exactly one `PushStatus` per input item, in order,
>   with correct terminal statuses for absent keys and size mismatches (validated by
>   unit tests against the mock transport).
> - **SC-002**: A second push to an already-connected host reuses the connection (no
>   new CM connect), avoiding the measured >2s establishment cost.
> - **SC-003**: A queue-pair error triggers exactly one reconnect-and-retry before
>   reporting `UnableToConnect`.
> - **SC-004** *(carries SC-005)*: With telemetry enabled, overhead is under 5% vs.
>   disabled. **NOTE: currently unmeasured for the push path — open task.**
>
> ## Assumptions
> - RDMA-capable hardware; isolated, trusted fabric.
> - The memory-tier pool is initialized before first push (base/size registerable).
> - `remote-lookup` provides the accept side, receive-buffer registration, and the
>   zyre control plane.
>
> ## Known limitations / follow-ups (from DESIGN.md)
> - Eviction race: `peek` returns ptr/size without pinning against eviction between
>   `peek` and write completion (data-freshness, not memory safety); pinning to be
>   added when integrating with `remote-lookup`.

**Confidence**: HIGH (interface + connection + DESIGN.md are consistent and tested)

**Action**:
- [ ] Approve and create spec (`/speckit.specify` seeded from this draft)
- [ ] Reject
- [ ] Modify

---

## Next steps

1. Approve/modify the two proposals above.
2. On approval, run `/speckit.sync.apply` (or `/speckit.specify` for spec-002) to
   materialize: set spec-001 Status → Superseded, write `specs/002-rdma-push-initiator/`.
3. Re-run `/speckit.sync.analyze` to confirm spec-002 aligns (target: 0 drifted,
   0 unspecced; SC-004 telemetry-overhead remains an open measurement task).
4. Record the cross-component boundary in `remote-lookup`'s own spec (accept side,
   receive-buffer registration, zyre control plane).
