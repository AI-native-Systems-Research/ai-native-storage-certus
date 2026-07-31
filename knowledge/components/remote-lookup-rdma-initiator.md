# remote-lookup-rdma-initiator

**Crate**: `remote-lookup-rdma-initiator`
**Path**: `components/remote-lookup-rdma-initiator/`
**Version**: 0.1.0
**Features**: `rdma` (real rdma-core transport), `telemetry` (connection/push counters)

## Description

Outbound RDMA push component — the data-holding (server) side of Certus's remote cache lookup. When another node requests a key, `remote-lookup` calls `push_async` to satisfy it by RDMA-writing the local value directly into the requester's pre-registered memory. Submission is asynchronous: the call enqueues a batch and returns, and a completion callback reports the per-item outcome once the writes land. Uses pool-based memory region registration and a connection table with one dedicated thread per remote host, which owns that host's queue pair and does both the posting and the reaping.

## Component Definition

```
RemoteLookupRdmaInitiatorComponent {
    version: "0.1.0",
    provides: [IRemoteLookupRdmaInitiator],
    receptacles: {
        logger: ILogger,
        memory_tier: IMemoryTier,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IRemoteLookupRdmaInitiator {
        fn push_async(&self, endpoint: &str, items: &[(CacheKey, RemoteRegion)], on_complete: PushCompletion) -> Result<(), RemoteLookupRdmaInitiatorError>;
        fn push(&self, endpoint: &str, items: &[(CacheKey, RemoteRegion)]) -> Result<Vec<PushStatus>, RemoteLookupRdmaInitiatorError>;
        fn connect(&self, endpoint: &str) -> Result<(), RemoteLookupRdmaInitiatorError>;
        fn disconnect(&self, endpoint: &str);
        fn disconnect_all(&self);
        fn set_local_peer_id(&self, peer: PeerId);
    }
}
```

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Connection events, warnings, timing |
| `memory_tier` | `IMemoryTier` | Yes | `peek(key)` for value lookup; `pool_info()` for RDMA MR registration |

## Key Semantics

- **Batch push model**: `push_async` takes `(CacheKey, RemoteRegion)` pairs and reports per-item `PushStatus` (Success, KeyNotFound, SizeMismatch, UnableToConnect) through its callback, in request order. `push` is a blocking convenience wrapper that submits and waits.
- **Completion guarantee**: an accepted batch (`Ok`) invokes `on_complete` exactly once on every outcome — success, per-item failure, connection loss, teardown. An `Err` drops the callback without invoking it, so a callback that releases resources must also release on drop. A batch rejected because the submit queue is full reports `UnableToConnect` for every item, possibly invoking the callback synchronously on the calling thread.
- **Callback owns the source-buffer lifetime**: the NIC keeps reading the local value buffers after submission returns, so whatever stops them being evicted or relocated (a dispatch-map read pin, in `remote-lookup`'s case) must be owned by the callback rather than released at the submission site.
- **Connection table, one thread per remote host**: each entry owns a dedicated thread that posts *and* reaps for that host's queue pair. `RdmaConn`'s queue pair is `Send` but not `Sync`, so keeping every access on one thread preserves that invariant with no new synchronization around it; submitters only enqueue on a bounded `MpscChannel`. The old transient Connecting/Disconnecting states are gone — there is no other thread to publish them to, and recovery is a phase of the thread's execution rather than an observable state.
- **Why asynchronous**: with a synchronous push the queue pair was held from first post to last completion, so exactly one operation per peer was ever in flight. Measured per-flow throughput was 3.24 GB/s on a 200 Gb/s (25 GB/s) RoCE link and did not respond to client concurrency, while the RDMA write window itself ran at ~16–20 GB/s (4 MiB posted and reaped in 212 µs mean / 185 µs p50) — roughly 80% of per-batch wall-clock was outside the RDMA path (zyre control plane, dispatch-map lookups, status round trip) with nothing able to overlap it.
- **Flow control**: send-queue credits bound in-flight writes to `PUSH_WINDOW` (128) and are topped up as completions retire, which is what pipelines successive batches instead of draining the send queue between them. Two further bounds: `SUBMIT_QUEUE_DEPTH` (256 batches per host) and `MAX_TRACKED_BATCHES` (64 batches held by the thread). A full queue fails fast rather than queueing, because every queued batch holds the submitter's read pins and a pinned entry cannot be evicted — an unbounded queue would let one sick peer stall the local memory tier.
- **Lazy connection with warm path**: connections established on first push; `connect` pre-warms so the caller does not pay the cold connect.
- **Recovery: destroy, then reconnect, then replay everything**: on a failed completion, a post failure, or a stall the thread destroys the queue pair *first* — that is the barrier guaranteeing the NIC has stopped reading pool memory — then reconnects and replays every batch that was outstanding, not just one, because a queue-pair error flushes all outstanding writes with a flush error and per-write blame is unknowable. Replay is idempotent. One repair per episode; a second consecutive failure fails all held batches as `UnableToConnect`. Blast radius is one peer, since a queue pair is keyed by requester endpoint.
- **Stall detection**: `connection::STALL_TIMEOUT` (2 s) applied to the age of the oldest posted write, replacing the old per-window `POLL_TIMEOUT_SECS` in `rdma.rs` (with several batches in flight there is no single window to time). Same rationale: `rdma_cm` leaves the hardware ACK timeout at its large default, so a write on a stale but nominally warm connection would otherwise burn ~15 s of retransmit before `RETRY_EXC`.
- **Teardown is a barrier**: `disconnect`/`disconnect_all` signal the host thread out of band (an atomic flag, since the submit queue can be full exactly when teardown matters most) and *join* it, so on return the queue pair is destroyed and every held batch has reported. `ConnectionTable::drop` does this for all hosts.
- **Transport seam**: `RdmaTransport`/`RdmaConn` traits. `RdmaConn` exposes `post_write(write, wr_id)` and non-blocking `poll_completions(max) -> Vec<Completion>`; the old blocking fixed-count `reap(count)` is gone, and `wr_id` is a slab slot index unique among outstanding writes rather than a position within a window. Mock transport for unit tests; real rdma-core behind `rdma` feature.
- **Pool-based MR**: memory-tier pool registered as single RDMA memory region per connection.
- **PeerId stamping**: `set_local_peer_id` stamps zyre UUID into `rdma_cm` connect `private_data` for peer correlation.

## Key Types

- `RemoteRegion { addr: u64, rkey: u32, length: u32 }` — target location in requester's memory
- `PushStatus` — `Success`, `KeyNotFound`, `SizeMismatch`, `UnableToConnect`
- `PushCompletion = Box<dyn FnOnce(Vec<PushStatus>) + Send>` — `push_async` completion callback; boxed so the interface stays object-safe, runs on the initiator's per-connection thread (so it must not block)
- `RemoteLookupRdmaInitiatorError` — `NotInitialized(String)`, `InvalidEndpoint(String)`
- `Completion { wr_id: u64, error: Option<String> }` (crate-internal, `connection.rs`) — one reaped work completion
