# remote-lookup-rdma-initiator

**Crate**: `remote-lookup-rdma-initiator`
**Path**: `components/remote-lookup-rdma-initiator/`
**Version**: 0.1.0
**Features**: `rdma` (real rdma-core transport), `telemetry` (connection/push counters)

## Description

Outbound RDMA push component — the data-holding (server) side of Certus's remote cache lookup. When another node requests a key, `remote-lookup` calls `push` to satisfy it by RDMA-writing the local value directly into the requester's pre-registered memory. Uses pool-based memory region registration and a connection table with per-host state machine.

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

- **Batch push model**: `push` takes `(CacheKey, RemoteRegion)` pairs, returns per-item `PushStatus` (Success, KeyNotFound, SizeMismatch, UnableToConnect).
- **Connection table**: per-host state machine (Disconnected→Connecting→Connected→Disconnecting). Outer table lock held briefly; per-slot mutex allows concurrent pushes to different hosts.
- **Lazy connection with warm path**: connections established on first `push`; `connect` pre-warms for sub-second hot path.
- **Single reconnect per batch**: on QP error, one reconnect attempt; remaining items short-circuit on failure.
- **Transport seam**: `RdmaTransport`/`RdmaConn` traits. Mock transport for unit tests; real rdma-core behind `rdma` feature.
- **Pool-based MR**: memory-tier pool registered as single RDMA memory region per connection.
- **PeerId stamping**: `set_local_peer_id` stamps zyre UUID into `rdma_cm` connect `private_data` for peer correlation.

## Key Types

- `RemoteRegion { addr: u64, rkey: u32, length: u32 }` — target location in requester's memory
- `PushStatus` — `Success`, `KeyNotFound`, `SizeMismatch`, `UnableToConnect`
- `RemoteLookupRdmaInitiatorError` — `NotInitialized`, `TransportError(String)`
