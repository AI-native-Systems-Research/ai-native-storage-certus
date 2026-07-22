# Remote Lookup RDMA Responder

The **accept** side of a remote RDMA lookup, owned by the *requesting* Certus
instance — the node that wants a value and offers local memory for a peer to
write it into. It is the passive counterpart of the outbound initiator
(`remote-lookup-rdma-initiator` / `IRemoteLookupRdmaInitiator`), driven for control
by the `remote-lookup` component and for lifecycle by the application mainline.

The responder is an **actor**: a dedicated thread runs an `rdma_cm` accept loop
that binds an ephemeral port on the mainline-supplied RoCE IPv4, accepts inbound
connections from serving peers, and keys a per-node connection table by the zyre
`PeerId` the initiator stamps into the connect `private_data`. The responder is
also the **registrar** for its memory tier: at `initialize()` it registers the
whole DRAM pool once with `ibv_reg_mr` (`REMOTE_WRITE`) and exposes the pool-wide
`rkey` via `local_region()`. Serving peers RDMA-**write** values one-sidedly into
that pool, so the responder's CPU never touches value bytes — it manages
**connections only**.

The load-bearing behavior is **teardown-before-reclaim**: on `Disconnect { node }`
the responder drives that peer's queue pair into the ERROR state (so late
one-sided writes are NAKed and cannot land) **before** emitting
`DisconnectAck { node }`; `remote-lookup` blocks on that ack before reclaiming the
peer's locked landing slots.

- **Interfaces** (in the `interfaces` crate):
  - `IRemoteLookupRdmaResponderAdmin` — `set_actor_cpu` / `set_bind_ip` /
    `initialize` / `signal_stop` / `shutdown` (driven by the mainline).
  - `IRemoteLookupRdmaResponder` — `open_control_channel` / `local_endpoint` /
    `local_region` (driven by `remote-lookup`).
  - Value types: `Endpoint`, `LocalRegion`, `ResponderCommand`, `ResponderEvent`,
    `ControlChannel`, `RemoteLookupRdmaResponderError`.
- **Receptacles**: `logger: ILogger` (optional; a missing logger is never an error);
  `memory_tier: IMemoryTier` (the pool the responder registers with `REMOTE_WRITE`).
- **Cargo feature `rdma`**: off by default (builds + unit-tests over the in-process
  mock seam with no rdma-core present); enable it for the real `rdma_cm` +
  `ibv_reg_mr` path. Mainline apps enable it; CI/default-members do not.
- **Features**: `telemetry` (off by default) — a zero-cost ZST collector when
  disabled; connection/teardown counters when enabled. See `benches/`.
- Not a workspace default member; built explicitly and (for the hardware path)
  requires rdma-core.

## Seam and status

All hardware-independent logic lives behind a CM seam
(`connection::CmListener` / `CmConnection`) so it is unit-testable and
benchmarkable without an RDMA NIC. The shipped implementation is the
**skeleton over the mock seam**: actor + control channel + lifecycle, the
`PeerId`-keyed `Active → Draining → Dead` state machine, teardown-before-ack
ordering, lossless (backpressure) event delivery, and telemetry — all covered by
`cargo test`. The production `rdma_cm` listener (real `bind`/`listen`/
`rdma_get_src_port`, `epoll` over the CM fd + eventfds, `private_data` read, real
QP teardown, NUMA pinning) is a hardware follow-up, exercised by an `#[ignore]`
loopback test. See `info/DESIGN.md`.

## Build / test / bench

```bash
cargo test  -p remote-lookup-rdma-responder                 # mock-seam logic (no hardware)
cargo test  -p remote-lookup-rdma-responder --features telemetry
cargo bench -p remote-lookup-rdma-responder --bench connection_telemetry -- --save-baseline off
cargo bench -p remote-lookup-rdma-responder --features telemetry --bench connection_telemetry -- --baseline off
```

See `specs/001-rdma-lookup-responder/` for the spec, plan, and task breakdown.
