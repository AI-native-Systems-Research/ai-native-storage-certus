# Phase 0 Research: RDMA Lookup Responder

The feature spec was clarified across two sessions (2026-07-10) and carries no
open `NEEDS CLARIFICATION` markers. The unknowns below are therefore *technical
design decisions* — how to realize the clarified behavior in Rust against
rdma-core, the component framework, and the sibling initiator's conventions.

---

## D1. Mirror the initiator's crate layout and seams

**Decision**: Reproduce `remote-lookup-rdma-initiator`'s module structure
(`lib.rs` / `ffi.rs` / `rdma.rs` / `connection.rs` / `telemetry.rs` / `wrapper.c`
/ `loopback_test.rs`, `build.rs`, `benches/`) in this crate, adapting each to the
accept side. Introduce a **mock CM seam** (traits `CmListener` + `CmConnection`)
that is the accept-side analog of the initiator's `RdmaTransport` + `RdmaConn`.

**Rationale**: The clarify session mandated cross-component consistency
(telemetry, benchmark). The initiator already proved a design where all
hardware-independent logic (connection table, state transitions, telemetry) lives
behind a trait seam and is unit-tested with a `MockTransport`/`MockConn`, while
the real rdma-core path is a thin `Real*` implementation exercised only by an
`#[ignore]` hardware test. The same split lets this crate pass `cargo test` in CI
(a default-member-excluded crate still builds/tests on demand) with zero hardware.

**Alternatives considered**:
- *One monolithic `lib.rs` with `#[cfg]`-gated hardware code.* Rejected: not
  unit-testable without hardware, diverges from the sibling, harder to review.
- *Reuse the initiator's `rdma.rs`/`ffi.rs` via a shared crate.* Rejected for v1:
  the FFI surfaces overlap but differ (accept vs connect); premature extraction
  couples two independently-versioned components. Revisit if a third RDMA
  component appears.

---

## D2. Accept-loop multiplexing and the testable "wait" seam

**Decision**: The production accept loop waits on `{rdma_cm channel fd, command
inbox, stop signal}` together via `epoll`. Because `component_core`'s
`SpscChannel` exposes **no raw fd**, pair the command inbox and the stop signal
each with an `eventfd(2)`: `remote-lookup`'s sends and `signal_stop()`/`shutdown()`
write the eventfd to wake `epoll`. The loop then drains the SPSC channel and the
CM channel by kind. Model this as a `CmListener::wait(...) -> Vec<CmEvent>`
method on the seam so the **mock** implementation can deliver injected connect
events and unblock on a command without any fd, while the **real** implementation
does the `epoll`. In the shipped skeleton the mock/loopback path is authoritative
and the poll-based placeholder in `run_accept_loop` is replaced by the seam.

**Rationale**: FR-004 + Story 4 + **SC-003** require that a `Disconnect` is
serviced promptly and never blocks behind a pending/absent accept. Multiplexing
the command inbox into the same wait is the whole point; an `eventfd` is the
standard Linux primitive to make a userspace queue `epoll`-visible. Keeping the
wait behind the seam makes SC-003 assertable over the mock (enqueue a command
with no pending connections, assert the ack returns promptly) with no hardware.

**Alternatives considered**:
- *Self-pipe instead of `eventfd`.* Works, but `eventfd` is cheaper (one 64-bit
  counter, no pipe buffer) and is the idiomatic choice.
- *Poll the command channel with a short timeout (the current skeleton's
  `try_recv` + `sleep`).* Rejected for production: adds latency up to the poll
  interval and burns CPU; acceptable only as the pre-seam placeholder.
- *Replace SPSC with an fd-backed channel.* Rejected: larger blast radius across
  the framework; the eventfd-pairing is local to this component.

---

## D3. QP→ERROR transition (the teardown-before-reclaim safety step)

**Decision**: Add `ibv_modify_qp` to the FFI (via `wrapper.c` if the inline/struct
layout is awkward to bind directly) and, on `Disconnect { node }`, set
`qp_attr.qp_state = IBV_QPS_ERR` with `attr_mask = IBV_QP_STATE`. Treat the call
as **infallible in practice and assert it** (`expect`/`assert!`): the transition
to `ERROR` is legal from any QP state and fails only on a fatal HCA/programming
fault, so a failure **fail-stops the process** rather than emitting a
`DisconnectAck`. `rdma_destroy_qp` afterward is best-effort — log its failure via
`ILogger`, do not fail-stop. Emit `DisconnectAck { node }` **only after** the
ERROR transition returns.

**Rationale**: This is FR-008 / SC-002 verbatim and the memory-safety linchpin: a
QP in `ERROR` NAKs late one-sided writes so they cannot land in reclaimed slots.
Asserting keeps `DisconnectAck` an *unconditional* guarantee — there is no
recoverable "error-and-withhold-ack" path (clarify session 2026-07-10).
`IBV_QPS_ERR` is already defined in the initiator's `ffi.rs` (value `6`); only
`ibv_modify_qp` needs adding.

**Alternatives considered**:
- *`rdma_disconnect` + `rdma_destroy_qp` only.* Rejected: `rdma_disconnect` is a
  graceful CM teardown whose completion is not synchronously ordered against
  in-flight remote writes the way the `ERROR` state is; the spec explicitly
  requires the QP be *unable to accept writes* before the ack.
- *Return an error variant on transition failure.* Rejected by the clarify
  session — a broken safety guarantee must fail-stop, not be reported and
  swallowed.

---

## D4. Ephemeral bind, device-by-IP, and reading the port back

**Decision**: In `initialize()`, resolve the local RoCE IPv4, build a
`sockaddr_in` with `sin_port = 0`, `rdma_bind_addr`, `rdma_listen`, then
`rdma_get_src_port(cm_id)` to read the OS-assigned port and publish
`Endpoint { ip, port }`. Never open a device by name; the NIC/NUMA path follows
from the bound IP's route (exactly as the initiator's `client_connect` lets
`rdma_cm` pick the device from the resolved address).

**Rationale**: FR-002 / FR-003 / SC-004. One instance per NUMA domain and
co-resident instances may share a NIC, so binding port 0 and reading it back is
what lets two responders advertise distinct `{ip, port}` without collision
(SC-004). The loopback/hardware test binds two listeners to confirm no collision.

**Alternatives considered**: A well-known port — rejected in the spec (co-resident
collision). Selecting the device by name — rejected (violates FR-002).

---

## D5. Identity correlation via connect `private_data`

**Decision**: On `RDMA_CM_EVENT_CONNECT_REQUEST`, read `event->param.conn.private_data`
/ `private_data_len`, interpret the bytes as the serving initiator's zyre UUID,
construct a `PeerId`, and key the connection table by it. Absent/malformed
`private_data` ⇒ still `rdma_accept`, but record the entry as **unidentified** and
emit `ConnectionEstablished { node: None }` — such an entry is reclaimable only via
`shutdown`. The mock seam injects `private_data` bytes directly so both branches
are unit-tested.

**Rationale**: FR-005 / FR-006 / SC-005 and the clarify answer that RDMA connect
is out-of-band from zyre and co-resident instances share a host IP, so the UUID in
`private_data` is the only reliable tie from an inbound QP to a zyre peer. The
UUID-stamping obligation on the *initiator* is a tracked cross-component follow-up,
not built here.

**Alternatives considered**: Correlate by source IP — rejected (co-resident
instances collide on one host IP). A side-channel zyre exchange — rejected (races
the RDMA connect; `private_data` rides the connect atomically).

---

## D6. Connection table, state machine, and idempotent teardown

**Decision**: A `HashMap<PeerId, ConnectionEntry>` internal to the actor (no lock
needed if only the accept-loop thread touches it — commands arrive on the inbox
and are handled *on* that thread). Each entry carries a state:
`Active → Draining → Dead`. `Disconnect { node }`: `Active → Draining`, QP→ERROR
(D3), destroy QP (best-effort), `→ Dead`, emit `DisconnectAck`. New connect for a
`Draining` node is **refused** (`rdma_reject`) so teardown is not raced (FR-007).
`Disconnect` for an unknown or already-`Dead` node is an **idempotent** no-op ack.
A second connect for an `Active` peer must not corrupt the existing entry.

**Rationale**: FR-007 / FR-008 / Story 2.3 / Story 3.2–3.3. Confining the table to
the actor thread removes cross-thread locking on the hot correlation path and
makes the state transitions trivially serial. Unidentified (`node: None`) entries
live in a side list (no `PeerId` key) reclaimed only on `shutdown`.

**Alternatives considered**: A `Mutex<HashMap>` shared with callers (like the
initiator's outer map) — unnecessary here because `remote-lookup` never holds a
connection handle; it only sends commands over the channel.

---

## D7. NUMA pinning of the accept-loop thread

**Decision**: `set_actor_cpu(cpu)` stores the target CPU; `initialize()` spawns the
accept-loop thread and, as its first action, pins itself with
`component_core::numa::CpuSet::from_cpu(cpu)` + `set_thread_affinity(&cpuset)` when
a CPU was set. A missing/failed pin is logged, not fatal (best-effort placement).

**Rationale**: FR-012 / Story 5.1. `component_core::numa` already provides
`CpuSet::from_cpu` and `set_thread_affinity` (used by the framework's actor
`with_cpu_affinity`), so no new syscall wrapping is needed.

**Alternatives considered**: Pin from the caller thread before spawn — rejected;
affinity must be set on the accept-loop thread itself.

---

## D8. Feature-gated telemetry + two-run overhead benchmark

**Decision**: Copy the initiator's `telemetry.rs` pattern exactly: a
`TelemetryCollector` that is a struct of `AtomicU64`s under `#[cfg(feature =
"telemetry")]` and a zero-sized `#[allow(clippy::unused_self)]` no-op under
`#[cfg(not(...))]`, with an identical method surface so call sites need no `#[cfg]`.
Metric set (FR-016): inbound connections accepted, identified vs unidentified
(`node: None`), teardowns (disconnect-acks emitted), accept-loop errors. Add a
Criterion `benches/connection_telemetry.rs` that drives accept→disconnect over the
mock CM seam and is compared feature-off vs feature-on via Criterion baselines,
mirroring `push_telemetry.rs`.

**Rationale**: FR-016 / SC-006 and the clarify decision to mirror the initiator.
The ZST-when-disabled shape is what makes "no cost when off" a compile-time fact,
and the two-run baseline workflow is how SC-006's < 5% budget is measured.

**Alternatives considered**: A runtime `bool` toggle — rejected (not zero-cost;
diverges from the sibling). A single-process on/off comparison — impossible for a
compile-time feature; the two-baseline workflow is required.

---

## D9. Skeleton-first, hardware-loop-later delivery

**Decision**: Land, over the mock CM seam: the actor + control channel + lifecycle
(`initialize`/`signal_stop`/`shutdown`, `AlreadyInitialized`/`NotInitialized`/
`ChannelClosed` paths), the `PeerId`-keyed state machine, the teardown-before-ack
ordering, telemetry, and the benchmark — all covered by `cargo test`. Defer to a
hardware follow-up (guarded by an `#[ignore]` loopback test): real
`rdma_bind_addr`/`rdma_listen`/`rdma_get_src_port`, `epoll` over the CM fd +
eventfds, real `private_data` read, real QP→ERROR/destroy, and NUMA pinning
verification.

**Rationale**: This is the spec's own "Known Limitations / Follow-ups" plan and
matches how the initiator was built and merged. It lets CI verify all logic
without an RDMA NIC while keeping the real path behind a single documented,
manually-run test.

**Alternatives considered**: Build the hardware loop first — rejected: unmergeable
in CI, and the safety-critical ordering logic is exactly what the mock seam lets us
test deterministically.

---

## Cross-cutting notes

- **rdma-core linkage** is identical to the initiator's `build.rs`
  (`pkg_config::probe_library` for `libibverbs`/`librdmacm`, `cc` compiles
  `wrapper.c`); the local box's install state is tracked in the
  `local-build-env-setup` memory.
- **No memory registration, no data touch** (FR-009 / FR-010) is preserved by
  construction: the responder holds only CM ids / QPs, never an `ibv_mr` and never
  a value buffer.
- **Security model**: network-level trust on an isolated fabric only (spec
  Assumptions), consistent with the initiator.
