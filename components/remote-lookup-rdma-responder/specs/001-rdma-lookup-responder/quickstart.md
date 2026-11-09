# Quickstart & Validation Guide: RDMA Lookup Responder

How to build, test, benchmark, and hardware-validate the responder. Scenarios map
to the spec's Acceptance Scenarios and Success Criteria (SC). Design details live
in [research.md](./research.md), [data-model.md](./data-model.md), and
[contracts/responder-control-interface.md](./contracts/responder-control-interface.md);
this guide is a run/validate reference, not an implementation walkthrough.

## Prerequisites
- Rust stable (workspace MSRV 1.75), Linux (RHEL/Fedora).
- **Unit tests, telemetry tests, and benchmarks need no RDMA hardware** — they run
  over the mock CM seam.
- The **hardware loopback test** needs an active RDMA device with a routable IPv4
  (RoCE/IB). rdma-core (`libibverbs`, `librdmacm`) must be installed; see the
  `local-build-env-setup` memory for this box's state.
- This crate is **not** a workspace default member (it links rdma-core); always
  target it explicitly with `-p remote-lookup-rdma-responder`.

## Build
```bash
# Hardware-free logic + interface (mock seam):
cargo build -p remote-lookup-rdma-responder

# With telemetry counters compiled in:
cargo build -p remote-lookup-rdma-responder --features telemetry
```

## Validation scenarios (no hardware)

Run the unit suite:
```bash
cargo test -p remote-lookup-rdma-responder
cargo test -p remote-lookup-rdma-responder --features telemetry   # telemetry wiring
```

| # | Scenario (spec ref) | What the test asserts | Command |
|---|--------------------|-----------------------|---------|
| 1 | Endpoint before/after init (US1, SC-001) | `local_endpoint()` → `NotInitialized` before `initialize()`; after `set_bind_ip(ip)` + `initialize()`, `{ip,port}` with the supplied IP and a non-placeholder port | `cargo test -p remote-lookup-rdma-responder endpoint` |
| 1b| Unset bind IP (FR-002a) | `initialize()` without a prior `set_bind_ip()` is accepted and defers to auto-detect (first active device); over the mock seam the endpoint IP is empty | `... initialize_without_bind_ip_defers_to_autodetect` |
| 2 | Double init / double open (Edge cases, FR-011) | 2nd `initialize()` → `AlreadyInitialized` (loop undisturbed); 2nd `open_control_channel()` → `ChannelClosed` | included in suite |
| 3 | Identity correlation (US2, SC-005) | connect w/ known UUID `private_data` → `ConnectionEstablished { node: Some(peer) }`; empty `private_data` → `node: None` | `... connect_identity` |
| 4 | Second connect for Active peer (US2.3) | existing entry's state machine not corrupted | included in suite |
| 5 | Teardown-before-ack ordering (US3, SC-002) | on `Disconnect{node}` the QP→ERROR transition is ordered **before** the single `DisconnectAck{node}`; entry ends `Dead` | `... teardown_order` |
| 6 | Idempotent disconnect (US3.2, FR-008) | `Disconnect` for unknown/`Dead` node → one `DisconnectAck`, no-op | `... disconnect_idempotent` |
| 7 | Refuse-while-draining (US3.3, FR-007) | new connect for a `Draining` node is refused | `... draining_refuses` |
| 8 | Prompt command servicing (US4, SC-003) | with **no** pending connects, an enqueued `Disconnect` is acked on an event-driven wake — asserted **structurally** (no intervening connection event, no poll cycle), not against a numeric wall-clock bound | `... prompt_command` |
| 9 | Lifecycle & stop (US5, FR-013) | `shutdown()` joins the thread & is idempotent; `signal_stop()` exits the loop without join | `... lifecycle` |
| 10| Telemetry counts (US6, FR-016) | accepted / identified / unidentified / teardowns / errors counters advance (feature on); ZST no-op when off | `--features telemetry ... telemetry` |

**Expected outcome**: all tests pass; scenario 5 is the safety-critical one — the
recorded order shows QP→ERROR strictly before the ack.

## SC-006 — telemetry overhead benchmark (< 5%, no hardware)
Two-run Criterion workflow over the mock CM seam (mirrors the initiator's
`push_telemetry`):
```bash
# Baseline: telemetry disabled (ZST no-op collector).
cargo bench -p remote-lookup-rdma-responder --bench connection_telemetry -- --save-baseline off

# Candidate: telemetry enabled (atomic counters on the accept/disconnect path).
cargo bench -p remote-lookup-rdma-responder --features telemetry --bench connection_telemetry -- --baseline off
```
**Expected outcome**: every case within +5% of the `off` baseline.

## SC-004 — co-resident distinct ports
Two responder instances bound on the same NIC advertise distinct ephemeral
`{ip,port}`. Covered logically in the unit suite via the seam; confirmed on real
hardware by the loopback test standing up two listeners.

## Hardware validation (real accept path, `#[ignore]`d)
Exercises real `rdma_bind_addr` / `rdma_listen` / `rdma_get_src_port`, `epoll` over
`{cm fd, command eventfd, stop eventfd}`, `private_data` read, and real QP→ERROR +
destroy — the follow-up work deferred from the skeleton.
```bash
cargo test -p remote-lookup-rdma-responder -- --ignored loopback
# Optionally pin the local RoCE IP (else auto-detected from the route):
CERTUS_RDMA_TEST_IP=10.0.0.102 cargo test -p remote-lookup-rdma-responder -- --ignored
```
**Expected outcome**: a real inbound connect is accepted and correlated to its
stamped UUID; a `Disconnect` drives the real QP to `ERROR` before the ack.

## Lint / docs gates (must pass)
```bash
cargo fmt --check
cargo clippy -p remote-lookup-rdma-responder --all-features -- -D warnings
cargo doc -p remote-lookup-rdma-responder --no-deps
```

## Interface-leakage check
Confirm `remote-lookup` reaches the responder only through its interfaces +
control channel, never the concrete struct:
```
/component-check-leakage
```
