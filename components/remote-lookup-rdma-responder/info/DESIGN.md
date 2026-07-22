# Design: Remote Lookup RDMA Responder

Design notes for the accept side of a remote RDMA lookup. The authoritative
requirements are in `specs/001-rdma-lookup-responder/spec.md`; this file records
the implementation shape and the decisions behind it.

## Role

Passive accept side owned by the *requesting* instance. `remote-lookup` advertises
this node's `{ip, port}` in a whisper; a serving initiator
(`remote-lookup-rdma-initiator`) connects in over `rdma_cm` and RDMA-**writes** the
value one-sidedly into the memory tier. The responder is the **registrar**: it
registers the whole DRAM pool once with `ibv_reg_mr` (`REMOTE_WRITE`) at
`initialize()` and exposes the pool-wide `rkey` via `local_region()`. It manages
only connections (queue pairs) and never touches the value bytes.

## Actor and control channel

A dedicated thread runs the accept loop. `remote-lookup` drives it over a
single-client `ControlChannel { command_tx, event_rx }`:

- Commands (`remote-lookup` → actor): `Disconnect { node }`.
- Events (actor → `remote-lookup`): `ConnectionEstablished { node: Option<PeerId> }`,
  `DisconnectAck { node }`, `Error { message }`.

Event delivery is **lossless** (FR-011a): the actor uses a blocking `send`
(backpressure) so a `DisconnectAck` is never dropped on a full channel; it cannot
deadlock because `remote-lookup` is the party awaiting that ack and so drains.

## The CM seam

All hardware-independent logic sits behind a seam, the accept-side analog of the
initiator's `RdmaTransport`/`RdmaConn`:

- `CmConnection::to_error()` — drive the RC queue pair to ERROR (asserted; the
  transition is always legal and fails only on a fatal HCA fault → fail-stop).
- `CmListener::next_events()` — the loop's single wait point; returns a batch of
  `CmEvent`s from `{cm channel, command inbox, stop}`. Blocks event-driven (a
  command `send` unparks it immediately — no poll cycle), satisfying SC-003.
- `CmEvent`: `ConnectRequest { private_data, conn }` / `Command(..)` / `Stop`.

`MockCmSeam` implements the seam for tests/benches (inject connects, deliver
commands over the real SPSC command channel). The production `RealCmSeam` (a
hardware follow-up) implements it with `rdma_bind_addr(port 0)` + `rdma_listen` +
`rdma_get_src_port`, an `epoll` over the `rdma_cm` fd + command/stop eventfds,
`private_data` reads, `rdma_accept`/`rdma_reject`, and `ibv_modify_qp` for the
QP→ERROR transition.

## Connection table & state machine

`ConnectionTable` keys identified connections by `PeerId` (parsed from the connect
`private_data`) and keeps a side-list of unidentified ones (`node: None`,
reclaimable only via `shutdown`). Each entry runs `Active → Draining → Dead`:

- **accept**: parse `private_data` → `PeerId`; insert `Active` (or replace on a
  reconnect, keeping the entry uncorrupted); a connect for a `Draining` peer is
  **refused** so teardown is not raced (FR-007).
- **disconnect** (FR-008, SC-002): `Active → Draining`, `to_error()` (QP→ERROR)
  **then** the caller emits the ack, then `→ Dead` and drop (best-effort QP
  destroy). Idempotent for unknown/dead peers.
- **teardown_all** (shutdown): error + drop every remaining connection.

## Bind IP and NUMA

`initialize()` binds the RoCE IPv4 supplied by the mainline via `set_bind_ip()`
(FR-002a — never auto-detected; deterministic on multi-NIC/NUMA hosts) with port 0
and reads back the ephemeral port. The device is selected by IP, never by name.
The accept-loop thread pins to `set_actor_cpu`'s NUMA node
(`component_core::numa`) as its first action (best-effort).

## Lifecycle

`signal_stop()` raises the stop flag and closes the retained command sender so the
loop exits cooperatively without a join; `shutdown()` does the same then joins the
thread and tears down all connections (idempotent). In the skeleton, stop relies
on the command channel closing to unblock the parked `recv`; the real path uses a
dedicated stop eventfd in the `epoll` set.

## Skeleton-first delivery

Shipped over the mock seam and fully CI-testable without a NIC: actor, control
channel, lifecycle, state machine, teardown ordering, lossless delivery,
telemetry, and the overhead benchmark. Deferred to the hardware follow-up:
`ffi.rs`, `wrapper.c`, `build.rs`, `RealCmSeam` in `rdma.rs`, and the `#[ignore]`
`loopback_test.rs`. Mirrors how the sibling initiator was built.
