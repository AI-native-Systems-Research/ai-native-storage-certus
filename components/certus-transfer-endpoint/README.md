# certus-transfer-endpoint

## Summary

A symmetric RDMA endpoint for explicit certus-to-certus KV cache transfer. A
destination reserves landing slots in its own memory, names **one** peer (chosen
externally, by llm-d), and asks it for a run of block keys; the source RDMA-writes
each value it holds straight into its slot and replies once with a per-key
outcome. There is no broadcast, no polling and no reap: exactly one peer is asked,
exactly once.

`CertusTransferEndpoint` provides `ITransferEndpoint` (`fetch`, the legacy
`push_async`/`push`, `register_landing_region`, `disconnect`, `stats`) and
`ITransferEndpointAdmin` (`configure`, `initialize`, `shutdown`). Its receptacles
are `logger` (optional), `dispatch_map` (atomic lookup-and-pin), `memory_tier`
(the pool, registered once for the endpoint's life) and, with `spdk`, `dispatcher`
(reads a block-tier entry into the memory tier's bounce reserve). Design documents:
`specs/001-rdma-cache-transfer/`; the invariants to review against: `CLAUDE.md`.

## The symmetric endpoint

One reliable-connected queue pair per *instance* pair (`"host:port"`, never a host
alone), and either end may post on it: an instance is a destination for one fetch
and a source for another on the same connection. The API is not symmetric — a
destination drives `fetch`, while a source answers inbound requests on its own
thread, so there is no `serve` method. Outbound and inbound books are kept apart,
since the requester allocates context ids and the same id can be live both ways.

## Three credit domains

Bounded separately, named separately, exhausted independently — and on exhaustion
the endpoint waits or chunks, never fails:

| domain | bounds | setting |
|---|---|---|
| send-queue credits | RDMA writes and messages in flight per connection, and so read pins held | `send_credits` |
| message credits | requests outstanding against a peer's posted receives; sized the reply reserve | `message_credits`, `inbound_quota` |
| bounce occupancy | block-tier entries being read or written from the memory tier's reserve | `block_readers`, `IMemoryTier::set_bounce_reserve` |

A fetch of any length is split into chunks of the peer's advertised `max_entries`
(340 per 8 KiB message), each its own message and credit.

## Features

Nothing is enabled by default, and the default build is the one CI runs.

| build | serves | transport |
|---|---|---|
| *(none)* | memory tier | in-process mock (a model of RC verbs, not a stub) |
| `rdma` | memory tier | libibverbs/librdmacm — never falls back to the mock |
| `spdk` | memory **and** block tier | as above |
| `telemetry` | extra counters beyond `stats()`, which counts in every build | as above |

Without `spdk`, a block-tier entry is reported `Miss`, never an error.

## Workspace membership

This crate **is** a workspace default member, unlike
`remote-lookup-rdma-initiator`/`-responder`, which link rdma-core unconditionally.
rdma-core sits behind `rdma` and SPDK behind `spdk`, so a plain `cargo build` /
`cargo test` needs neither a NIC nor an NVMe device.

## Building and testing

```sh
cargo test -p certus-transfer-endpoint                       # default: mock, no hardware
cargo clippy -p certus-transfer-endpoint --all-targets --all-features -- -D warnings
cargo doc -p certus-transfer-endpoint --no-deps

# Hardware: a local RoCE address. Single-host loopback, two ports of one address.
CERTUS_RDMA_TEST_IP=10.0.0.102 \
    cargo test -p certus-transfer-endpoint --features rdma -- --ignored
```

The two-node test (`loopback_test::two_node`) runs one role per node; its doc
comment has the commands, and `specs/001-rdma-cache-transfer/evidence/` the run.

## Benchmarks

Both go through `ITransferEndpoint` only, as a caller would, over the build's own
transport. Over verbs they need `CERTUS_RDMA_TEST_IP` and are skipped without it.

```sh
cargo bench -p certus-transfer-endpoint --bench entry_push       # mock
CERTUS_RDMA_TEST_IP=10.0.0.102 numactl -N0 -m0 \
    cargo bench -p certus-transfer-endpoint --features rdma --bench entry_push
```

* `entry_push` — `push` of N memory-tier entries: the per-entry path of FR-008.
* `message_codec` — a 340-entry `fetch`, all-miss (the codec, credits and thread
  hops, with no writes) beside all-hit (the difference is the writes), and a
  1,536-entry all-miss fetch (five messages).

Measured 2026-10-20 on node2, single-host loopback over `mlx5_0` (200 Gb/s RoCE),
NUMA 0, default credits:

| benchmark | mock | verbs |
|---|---|---|
| push 1 × 64 KiB | 21.6 µs | 15.6 µs |
| push 64 × 64 KiB | 149 µs | 403 µs (9.7 GiB/s) |
| push 340 × 64 KiB | 1.69 ms | 2.05 ms (10.1 GiB/s) |
| push 1 × 2.25 MiB | — | 228 µs (9.6 GiB/s) |
| push 64 × 2.25 MiB | — | 14.4 ms (9.8 GiB/s) |
| fetch 340, all miss | 46.9 µs | 30.2 µs |
| fetch 340, all hit (64 KiB) | 1.64 ms | 2.52 ms |
| fetch 1,536, all miss | 90.0 µs | 74.7 µs |

* **The message path is cheap**: a 340-entry request answered with no writes costs
  ~30 µs end to end over verbs, ~0.09 µs an entry. The all-hit fetch is almost
  entirely its writes.
* **Loopback is not the fabric.** `ib_write_bw` on the same loopback reaches
  12.9-13.2 GiB/s on one QP and 15.0 GiB/s on four, against 9.6-10.1 GiB/s here;
  a loopback write crosses one NIC twice. Across node2 → node5, one `ib_write_bw`
  QP reaches 17.7 GiB/s and the endpoint's own two-node fetch ~17.8 GiB/s
  (19.1 GB/s, `evidence/t056-two-node.md`) — at the fabric's rate, on one
  connection. Quote loopback numbers as loopback.
* **Not yet a throughput study.** One run each; raising `send_credits` from 8 to 64
  did not move the 64 KiB loopback number, so the cap there is not the credit
  window.
