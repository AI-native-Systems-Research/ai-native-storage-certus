# Phase 1 Data Model: RDMA Lookup Responder

Entities the responder exposes across its interface plus the internal state it
keeps. Public value types already live in
`components/interfaces/src/iremote_lookup_rdma_responder.rs`; internal types are
new in this crate. Fields, invariants, and transitions are traced to the spec's
Functional Requirements (FR) and Success Criteria (SC).

---

## Public (interface) entities

### Endpoint
The bound listening endpoint advertised by `remote-lookup` in whispers.

| Field | Type   | Notes |
|-------|--------|-------|
| `ip`  | `String` | Local RoCE IPv4 the listener is bound to; the mainline's `set_bind_ip()` value when supplied, else **auto-detected** (first active device's IPv4). Pins the NIC/NUMA path (device never chosen by name). |
| `port`| `u16`    | **Ephemeral** — assigned by the OS at `rdma_bind_addr` (port 0) and read back via `rdma_get_src_port`. |

- **Invariant**: after `initialize()`, `port` is non-zero/non-placeholder
  (FR-003, SC-001). Two co-resident instances yield distinct ports (SC-004).
- Display: `"{ip}:{port}"`.

### LocalRegion
The pool-wide memory region the responder registers at `initialize()` and exposes
via `local_region()` for `remote-lookup` to advertise (FR-010).

| Field | Type | Notes |
|-------|------|-------|
| `addr` | `u64` | Base virtual address of the registered memory-tier pool. |
| `rkey` | `u32` | Pool-wide remote key authorizing inbound one-sided `REMOTE_WRITE`. |
| `length` | `usize` | Pool length in bytes (`usize`, since a pool may exceed 4 GiB). |

- **Invariant**: registered once with a stable `rkey` for the process lifetime; the
  per-slot bound is enforced in software by `remote-lookup`, not by the NIC.

### PeerId
A zyre node identity (UUID). Owned by `remote-lookup`; the responder uses it as
the connection-table key, resolved from the connect `private_data` (FR-005).
Newtype over `String` (`interfaces::PeerId`).

### ResponderCommand  *(control → actor)*
| Variant | Fields | Meaning |
|---------|--------|---------|
| `Disconnect` | `node: PeerId` | Tear down `node`'s QP (QP→ERROR, then destroy) **before** acking, so late one-sided writes cannot land after slot reclaim (FR-008). |

No per-value command exists — writes are one-sided (FR-009).

### ResponderEvent  *(actor → control)*
| Variant | Fields | Meaning |
|---------|--------|---------|
| `ConnectionEstablished` | `node: Option<PeerId>` | Inbound connect accepted; `Some` if `private_data` carried a valid UUID, `None` if absent/malformed (FR-005/FR-006, SC-005). |
| `DisconnectAck` | `node: PeerId` | **Unconditional** guarantee that `node`'s late writes can no longer land; reclaim is now safe (FR-008, SC-002). |
| `Error` | `message: String` | A non-fatal accept-loop error (distinct from fail-stop faults). |

### ControlChannel
Single-client `{ command_tx: Sender<ResponderCommand>, event_rx:
Receiver<ResponderEvent> }` handed to `remote-lookup` by
`open_control_channel()`. A second `open_control_channel()` ⇒ `ChannelClosed`
(FR-011).

### RemoteLookupRdmaResponderError  *(method-level failures)*
`NotInitialized` · `AlreadyInitialized` · `Bind` · `ChannelClosed` · `Internal`.
Per-connection outcomes are reported via `ResponderEvent`, never here.

---

## Internal entities (this crate)

### ConnectionEntry
One inbound peer connection. Identified entries are keyed by `PeerId`;
unidentified ones are held in a side collection (no key), reclaimable only on
`shutdown` (FR-006).

| Field | Type | Notes |
|-------|------|-------|
| `state` | `ConnState` | `Active → Draining → Dead` (below). |
| `conn`  | `Box<dyn CmConnection>` | The CM id + QP behind the seam; carries the QP→ERROR + destroy ops. Never an `ibv_mr` and never a value buffer (FR-009/FR-010). |

### ConnState  *(state machine — the memory-safety linchpin)*
```
        accept (CONNECT_REQUEST, valid QP)
   ─────────────────────────────────────────►  Active
                                                  │
                        Disconnect{node}          │  (QP → ERROR, then destroy QP best-effort)
                                                  ▼
                                               Draining ──────────►  Dead
                                                  ▲                    │
              new connect for a Draining node ────┘ (refused: rdma_reject, FR-007)
                                                                       │
                          Disconnect{node} on Dead/unknown ────────────┘ (idempotent no-op ack, FR-008)
```

Transition rules:
- **Active → Draining**: entered on `Disconnect { node }` for an `Active` node.
- **QP→ERROR (in Draining)**: asserted; fail-stops the process on failure. It is
  ordered strictly **before** `DisconnectAck` (FR-008, SC-002).
- **destroy QP → Dead**: `rdma_destroy_qp` is best-effort; failure logged via
  `ILogger`, not fatal.
- **`DisconnectAck { node }`** emitted only on reaching `Dead` (or immediately for
  an unknown/`Dead` node — idempotent).
- **New work while Draining**: refused (`rdma_reject`) — teardown is never raced
  (FR-007, Story 3.3).
- **Second connect while Active**: handled without corrupting the existing entry
  (Story 2.3).

### CM seam (testability boundary)
Accept-side analog of the initiator's `RdmaTransport`/`RdmaConn`:

| Trait | Role | Real impl | Mock impl |
|-------|------|-----------|-----------|
| `CmListener` | Bind/listen, then `wait()` on `{cm fd, command inbox, stop}` and yield `CmEvent`s | `RealCmSeam` (rdma-core: `rdma_bind_addr`/`rdma_listen`/`rdma_get_src_port`/`epoll`) | `MockCmSeam` (injects connect events, `private_data`, unblocks on commands) |
| `CmConnection` | Carries one accepted QP; `to_error()` (QP→ERROR) + drop-destroy | `RealCmConn` | `MockCmConn` |

`CmEvent` (internal): `ConnectRequest { private_data: Option<Vec<u8>>, conn }` ·
`Command(ResponderCommand)` · `Stop`. This is what makes SC-003/SC-005 and the
state machine unit-testable and the SC-006 benchmark hardware-free.

### Command-inbox bridge thread  *(real seam only; backfilled 2026-08-20)*
The `command inbox` in FR-004 is a lock-free SPSC channel, which has **no pollable
fd**, so the real `epoll`-based accept loop cannot wait on it directly. `RealCmSeam`
therefore spawns a dedicated bridge thread `rdma-responder-cmd-bridge`
(`src/rdma.rs:358-373`) that blocks on the SPSC receiver and, for each dequeued
`ResponderCommand`, pushes it onto an internal `Mutex<VecDeque<..>>` and signals the
command `eventfd` (`TAG_CMD`) that the accept loop's `epoll` set watches. The accept
loop then drains that queue when the eventfd fires.

- **Role**: SPSC→eventfd adapter realizing FR-004's "command inbox" wait arm; adds no
  externally visible behavior and preserves prompt, event-driven command servicing
  (FR-004, SC-003).
- **Lifecycle**: owned by `RealCmSeam` (`bridge: Some(JoinHandle)`); ends when the
  command channel closes on shutdown.
- **Mock seam**: has no bridge thread — `MockCmSeam` delivers injected commands
  directly — so this entity exists only on the real (`rdma`-feature) path.

### TelemetryCollector  *(feature-gated)*
ZST no-op when `telemetry` is off (compile-away methods); atomic counters when on
(FR-016). Metrics:

| Counter | Incremented on |
|---------|----------------|
| `connections_accepted` | every accepted inbound connect |
| `connections_identified` | accept with a valid `PeerId` from `private_data` |
| `connections_unidentified` | accept with `node: None` |
| `teardowns` | each `DisconnectAck` emitted |
| `accept_loop_errors` | each non-fatal accept-loop error (`ResponderEvent::Error`) |

- **Invariant** (SC-006): enabling the feature adds < 5% overhead vs the disabled
  build on the accept/disconnect path, measured by the two-run Criterion
  benchmark.

---

## Live actor state (`lib.rs`)
Already present in the skeleton; the model records it for completeness:

| Field | Type | Role |
|-------|------|------|
| `actor_cpu` | `Mutex<Option<usize>>` | NUMA target set by `set_actor_cpu` before `initialize` (FR-012). |
| `bind_ip` | `Mutex<Option<String>>` | RoCE IPv4 set by `set_bind_ip` before `initialize`; `None`/empty ⇒ auto-detect the first active device (FR-002a). |
| `state` | `Mutex<Option<ActorState>>` | `Some` iff initialized (drives `NotInitialized`/`AlreadyInitialized`). |
| `endpoint` | `OnceLock<Endpoint>` | Published bound endpoint (FR-003). |
| `local_region` | `OnceLock<LocalRegion>` | Published pool-wide region `{addr, rkey, length}` registered at `initialize` (FR-010). |
| `logger` | receptacle `ILogger` | Optional; missing logger is never an error (FR-014). |
| `memory_tier` | receptacle `IMemoryTier` | Source of the pool (`pool_info()`) registered with `REMOTE_WRITE` at `initialize` (FR-010). |

`ActorState` holds the not-yet-handed-out `command_tx`/`event_rx` (single-client),
the cooperative `stop` flag/eventfd, and the accept-loop `JoinHandle` (joined by
`shutdown`, FR-013).
