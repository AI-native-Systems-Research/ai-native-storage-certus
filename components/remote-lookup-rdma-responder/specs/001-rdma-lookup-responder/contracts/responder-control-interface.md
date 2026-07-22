# Contract: Responder Control Interface & Teardown Protocol

The responder exposes its behavior through two Rust component interfaces (defined
in `interfaces` and discovered at runtime via `IUnknown`) plus a control-channel
message protocol and one ordering guarantee. This is the full contract between the
responder, the application mainline, and `remote-lookup`.

Interface source of truth:
`components/interfaces/src/iremote_lookup_rdma_responder.rs` (already landed).

---

## 1. `IRemoteLookupRdmaResponderAdmin` — lifecycle (driven by the mainline)

```rust
fn set_actor_cpu(&self, cpu: usize);
fn set_bind_ip(&self, ip: String);
fn initialize(&self) -> Result<(), RemoteLookupRdmaResponderError>;
fn signal_stop(&self);
fn shutdown(&self) -> Result<(), RemoteLookupRdmaResponderError>;
```

| Method | Preconditions | Postconditions / guarantees | Errors |
|--------|---------------|-----------------------------|--------|
| `set_actor_cpu(cpu)` | called before `initialize()` | accept-loop thread will pin to `cpu` (FR-012) | — (infallible) |
| `set_bind_ip(ip)` | called before `initialize()` | records the local RoCE IPv4 to bind, overriding auto-detection (FR-002a) | — (infallible) |
| `initialize()` | not already initialized | binds ephemeral port on the effective RoCE IPv4 (the `set_bind_ip()` value, else the first active device's IPv4), `rdma_listen`, reads port via `rdma_get_src_port`, starts accept loop; `local_endpoint()` now returns the bound `{ip,port}` (FR-002/002a/003) | `AlreadyInitialized` if called twice (running loop undisturbed); `Bind` if the effective IP is missing/unusable (incl. no active device found) or listen fails |
| `signal_stop()` | — | accept loop exits cooperatively **without** join (FR-013); idempotent | — |
| `shutdown()` | — | stops + **joins** accept thread, tears down all remaining connections and the listener; idempotent (2nd call is a no-op) (FR-013) | `Internal` if the accept thread panicked |

---

## 2. `IRemoteLookupRdmaResponder` — runtime control (driven by `remote-lookup`)

```rust
fn open_control_channel(&self) -> Result<ControlChannel, RemoteLookupRdmaResponderError>;
fn local_endpoint(&self) -> Result<Endpoint, RemoteLookupRdmaResponderError>;
fn local_region(&self) -> Result<LocalRegion, RemoteLookupRdmaResponderError>;
```

| Method | Preconditions | Postconditions | Errors |
|--------|---------------|----------------|--------|
| `open_control_channel()` | initialized; not already opened | returns the single-client `{command_tx, event_rx}` (FR-011) | `NotInitialized` before init; `ChannelClosed` on a 2nd call |
| `local_endpoint()` | initialized | returns bound `{ip, port}` with the OS-assigned ephemeral port (FR-003, SC-001) | `NotInitialized` before init |
| `local_region()` | initialized | returns the pool-wide `LocalRegion { addr, rkey, length }` registered at `initialize()` (FR-010) | `NotInitialized` before init |

---

## 3. Control-channel message protocol

**Commands (`remote-lookup` → responder)** — `ResponderCommand`:
- `Disconnect { node: PeerId }`

**Events (responder → `remote-lookup`)** — `ResponderEvent`:
- `ConnectionEstablished { node: Option<PeerId> }`
- `DisconnectAck { node: PeerId }`
- `Error { message: String }`

Protocol rules:
1. Each accepted inbound connect emits exactly one `ConnectionEstablished`.
   `node = Some(peer)` iff the connect `private_data` carried a valid zyre UUID;
   otherwise `node = None` (FR-005/006, SC-005).
2. Each `Disconnect { node }` is answered with **exactly one** `DisconnectAck {
   node }` (SC-002), including the idempotent no-op case (unknown/already-dead
   node) (FR-008).
3. `Error { message }` reports a **non-fatal** accept-loop error. Fatal
   safety-guarantee faults (§4) are **not** reported here — they fail-stop.
4. There is **no data-path message** — value bytes arrive by one-sided RDMA write
   into the responder-registered pool, out of band from this channel (FR-009/010).

---

## 4. The teardown-before-reclaim ordering guarantee (load-bearing)

On `Disconnect { node }` for a live peer, the responder MUST, in this order:

```
Active ─► Draining
   1. QP → ERROR           (ibv_modify_qp, qp_state = IBV_QPS_ERR; ASSERTED — fail-stop on failure)
   2. destroy QP           (rdma_destroy_qp; best-effort — log on failure, not fatal)
      ─► Dead
   3. emit DisconnectAck { node }
```

Guarantees:
- **G1 (ordering, SC-002)**: step 1 is observably ordered **before** step 3. Once
  a QP is in `ERROR` it NAKs late one-sided writes, so they cannot land in slots
  `remote-lookup` is about to reclaim.
- **G2 (unconditional ack)**: `DisconnectAck` is emitted iff step 1 succeeded.
  Step 1 fails only on a fatal HCA/programming fault, which **fail-stops the
  process** — there is no "error-and-withhold-ack" path. Therefore receiving
  `DisconnectAck` is an unconditional guarantee that reclaim is safe (FR-008).
- **G3 (no race)**: a new connect for a `Draining`/`Dead` node is refused so
  teardown is never raced by new work (FR-007).
- **G4 (idempotent)**: `Disconnect` for an unknown or already-`Dead` node is a
  no-op that still emits `DisconnectAck` (FR-008).

`remote-lookup`'s obligation: block on `DisconnectAck { node }` before reclaiming
that node's locked landing slots.

---

## 5. Cross-component / environmental contract
- **Initiator obligation** (tracked against the initiator, not here): every
  serving initiator stamps its own zyre UUID into the connect `private_data`.
  Unstamped connects are tolerated as `node: None` (reclaimable only via
  `shutdown`).
- **Registrar role**: the responder itself registers the whole memory-tier pool
  once (read via the `memory_tier` receptacle's `IMemoryTier::pool_info()`) with
  `ibv_reg_mr` (`REMOTE_WRITE`) at `initialize()`, exposing the pool-wide `rkey`
  through `local_region()`; no per-request `ibv_reg_mr` (FR-010).
- **Diagnostics**: routed through an optional `ILogger` receptacle; a missing
  logger never turns an operation into an error (FR-014).
- **Device selection**: never by name — binding by IP implies the NIC/NUMA path
  (FR-002).

---

## 6. Telemetry contract (feature `telemetry`)
When enabled, the collector records: connections accepted, identified vs
unidentified (`node: None`), teardowns (disconnect-acks emitted), and accept-loop
errors (FR-016). When disabled it is a zero-sized no-op with an identical method
surface (no call-site cost). Overhead budget: < 5% vs the disabled build (SC-006).
