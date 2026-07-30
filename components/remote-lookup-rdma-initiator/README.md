# Remote Lookup Initiator

Outbound RDMA "push" component. Given a remote host endpoint and a batch of
`(key, remote-region)` pairs, it connects to the host (reusing an established
connection), looks each key up in the **local memory tier**, and — when the key
is present and its size matches the region — RDMA-writes the value directly into
the remote host's memory.

It is the *server-side* half of a remote lookup: the requesting node sends its
keys and the descriptors of the memory it wants the data written into (over the
zyre control plane handled by the `remote-lookup` component), and this component
fulfills the request by pushing the data out over RDMA.

## Architecture

```
This Node (has the data)                 Remote Node (wants the data)
┌───────────────────────────┐            ┌──────────────────────────┐
│  remote-lookup  ──push()─▶ │            │  remote-lookup           │
│  remote-lookup-rdma-initiator    │            │  (rdma_cm accept +       │
│  ┌──────────────────────┐  │            │   registered recv bufs)  │
│  │  ConnectionTable     │──rdma_connect─▶                          │
│  │  (per-host QP,       │  │            │  ┌────────────────────┐  │
│  │   state machine)     │──RDMA Write──▶│  │  Result Buffers    │  │
│  └──────────┬───────────┘  │            │  │  (addr + rkey)     │  │
│             │ peek(key)     │            │  └────────────────────┘  │
│  ┌──────────▼───────────┐  │            └──────────────────────────┘
│  │  IMemoryTier (pool)  │  │
│  └──────────────────────┘  │
└───────────────────────────┘
```

### Module Layout

| Module          | Role                                                                                                          |
| --------------- | ------------------------------------------------------------------------------------------------------------- |
| `ffi.rs`        | Raw FFI bindings to libibverbs and librdmacm                                                                  |
| `wrapper.c`     | C helpers for inline ibverbs functions (`poll_cq`, `rdma_write`)                                              |
| `rdma.rs`       | Safe wrappers: `client_connect`, `RdmaConnection`, `MemoryRegion` (RAII), QP-health check, `post_write_from_pool` + `reap` |
| `connection.rs` | `ConnectionTable` + per-host state machine, the `RdmaTransport`/`RdmaConn` seam (real + test mock), `ItemPlan` |
| `lib.rs`        | `RemoteLookupRdmaInitiatorComponent` implementing `IRemoteLookupRdmaInitiator` (`push`/`disconnect`/`disconnect_all`/`set_local_peer_id`)    |
| `telemetry.rs`  | Feature-gated (`telemetry`) atomic counters                                                                   |

### Interface

`IRemoteLookupRdmaInitiator` (in the `interfaces` crate):

- `push(endpoint, items) -> Result<Vec<PushStatus>, RemoteLookupRdmaInitiatorError>` —
  connect (reuse/repair), look up each key locally, RDMA-write matches. Returns
  one `PushStatus` (`Success` / `UnableToConnect` / `KeyNotFound` / `SizeMismatch`)
  per item, in order.
- `disconnect(endpoint)` — tear down one host's connection (idempotent).
- `disconnect_all()` — tear down all connections.
- `set_local_peer_id(peer)` — supply this node's zyre `PeerId`; it is stamped into
  the `rdma_cm` connect `private_data` on every outbound connection so the remote
  responder can correlate the inbound queue pair to this peer (teardown-before-reclaim).
  Call once before the first `push`.

### Connection model

Connections are kept in a table keyed by `"ip:port"`. A host absent from the
table is *disconnected*; an entry is *connecting*, *connected*, or
*disconnecting*. Establishing a RoCE/CM connection takes seconds, so connections
are reused across calls. Pushes to different hosts run concurrently; pushes to
the same host serialize on that host's slot, which is also what makes them queue
behind an in-progress reconnect rather than pile onto a dead queue pair.

### Write windowing

A push posts up to `PUSH_WINDOW` (128, the queue pair's send/completion depth)
writes before waiting on any completion, then reaps them all. Waiting on each
write individually caps a flow near 9% of a 200 Gb/s link, because a 64 KiB write
is ~2.6 µs of wire time but ~28 µs of post/poll overhead. Batches larger than the
window are split into successive windows.

Recovery is per window, not per write: a failing RDMA_WRITE drives the queue pair
into the error state, which flushes every other outstanding request, so per-write
blame cannot be assigned. On a lost window the connection is torn down, rebuilt
once, and the **entire window replayed**. That replay is safe because a push is
idempotent until its status is reported — the requester's landing buffers stay
reserved and unpublished, and it cannot reclaim them without first tearing down
the connection (SC-005, teardown-before-reclaim). A window that fails again after
the rebuild reports `UnableToConnect` for its keys.

### Security Model

Network-level trust only. No application-level authentication. Assumes an
isolated RDMA fabric not reachable from untrusted networks.

## Prerequisites

- Rust stable toolchain (MSRV 1.75)
- **For the real transport only** (`--features rdma`): Linux with an RDMA-capable
  NIC (InfiniBand or RoCE) and the rdma-core userspace libraries —
  `libibverbs-dev`, `librdmacm-dev` (Debian/Ubuntu) or `rdma-core-devel`
  (RHEL/Fedora). Without the feature the crate needs no rdma-core to build.

This crate is **not** a workspace default member; it is built explicitly. It is
SPDK-orthogonal (it uses only `IMemoryTier`/`ILogger`, neither of which needs the
`spdk` feature).

## Build

```bash
# Default: builds over the in-process mock transport — no rdma-core required.
cargo build -p remote-lookup-rdma-initiator
cargo build -p remote-lookup-rdma-initiator --features telemetry

# Real rdma-core transport (links libibverbs/librdmacm):
cargo build -p remote-lookup-rdma-initiator --features rdma

# As part of certus-server-yaml (full-remote profile requires the rdma feature)
CERTUS_PROFILE=full-remote cargo build -p certus-server-yaml --features rdma
```

## Test

```bash
# Unit tests — connection-table state machine + status logic against a mock RDMA
# transport. No RDMA hardware or rdma-core required.
cargo test -p remote-lookup-rdma-initiator

cargo clippy -p remote-lookup-rdma-initiator -- -D warnings
cargo fmt -p remote-lookup-rdma-initiator --check
```

### Hardware data-path test (single-host loopback)

`src/loopback_test.rs` contains a `#[cfg(test)]`, `#[ignore]`d integration test
that exercises the real outbound path (`client_connect` → register the pool MR →
`rdma_write_from_pool`) end-to-end on one machine. It stands up a minimal
`rdma_cm` responder as **test-only scaffolding** (the accept side otherwise lives
in `remote-lookup`), pre-registers a destination buffer, then pushes a payload
from a source buffer into it over RDMA and verifies the bytes arrived.

It requires an active RDMA device with a routable IPv4 (RoCE or IB). The device
is selected implicitly by `rdma_cm` from that IP's route — as in production —
so nothing is opened by name. The IP is auto-detected from
`/sys/class/infiniband`; override it with `CERTUS_RDMA_TEST_IP`.

```bash
# Run the loopback test (needs the rdma feature; auto-detects the local RoCE/IB IPv4):
cargo test -p remote-lookup-rdma-initiator --features rdma -- --ignored loopback

# Pin the IP explicitly:
CERTUS_RDMA_TEST_IP=10.0.0.102 cargo test -p remote-lookup-rdma-initiator -- --ignored
```

If no active RDMA IPv4 is found the test prints a skip notice and passes (it is
`#[ignore]`d, so it never runs in the default `cargo test`).

## Benchmark

`benches/push_telemetry.rs` (Criterion) measures the `push` path over a mock RDMA
transport, so it needs no hardware. It characterizes **SC-004** — the cost of the
`telemetry` feature, which is a small fixed set of `Relaxed` atomic counters when
on and a zero-sized no-op when off. Because telemetry is a compile-time feature,
the on/off comparison is two runs against a saved baseline:

```bash
# Baseline: telemetry disabled (no-op collector).
cargo bench -p remote-lookup-rdma-initiator --bench push_telemetry -- --save-baseline off

# Candidate: telemetry enabled (atomic counters on the push path).
cargo bench -p remote-lookup-rdma-initiator --features telemetry --bench push_telemetry \
    -- --baseline off
```

The second run prints the percentage change per case. Interpret it in absolute
terms, not as a raw percentage: the mock push is a ~200–700 ns no-op, so the
handful of unavoidable atomic counters (~13 ns/push, measured 2026-07-15) reads
as 6–13% *of the mock* — while against a real one-sided RDMA write (µs–ms) the
same cost is <0.1%. SC-004 holds because that overhead is a small fixed constant
and zero when the feature is off, not because it clears a 5%-of-mock bar (it does
not, by construction).
