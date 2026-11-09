# remote-lookup-rdma-responder

**Crate**: `remote-lookup-rdma-responder`
**Path**: `components/remote-lookup-rdma-responder/`
**Version**: 0.1.0
**Features**: `rdma` (real rdma-core path), `telemetry` (connection counters)

## Description

The accept side (passive/responder) of the RDMA remote-lookup subsystem. Belongs to the **requesting** Certus node — the one that wants a value. Accepts inbound RDMA connections from serving peers who RDMA-write values one-sidedly into the requester's pre-registered memory tier. The responder's CPU never touches data; it manages only connections and memory registration.

## Component Definition

```
RemoteLookupRdmaResponderComponent {
    version: "0.1.0",
    provides: [IRemoteLookupRdmaResponder, IRemoteLookupRdmaResponderAdmin],
    receptacles: {
        logger: ILogger,
        memory_tier: IMemoryTier,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IRemoteLookupRdmaResponder {
        fn open_control_channel(&self) -> Result<ControlChannel, RemoteLookupRdmaResponderError>;
        fn local_endpoint(&self) -> Result<Endpoint, RemoteLookupRdmaResponderError>;
        fn local_region(&self) -> Result<LocalRegion, RemoteLookupRdmaResponderError>;
    }
}

define_interface! {
    pub IRemoteLookupRdmaResponderAdmin {
        fn set_actor_cpu(&self, cpu: usize);
        fn set_bind_ip(&self, ip: String);
        fn initialize(&self) -> Result<(), RemoteLookupRdmaResponderError>;
        fn signal_stop(&self);
        fn shutdown(&self) -> Result<(), RemoteLookupRdmaResponderError>;
    }
}
```

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Diagnostic logging |
| `memory_tier` | `IMemoryTier` | Yes | Pool registration: `pool_info()` provides base+size for `ibv_reg_mr(REMOTE_WRITE)` |

## Key Semantics

- **Actor model**: dedicated `rdma-responder-accept` thread, NUMA-pinned. Communication via SPSC `ControlChannel`.
- **CM Seam (testability)**: `CmListener`/`CmConnection` trait abstraction. `MockCmSeam` (default) for unit tests; `RealCmSeam` (behind `rdma` feature) for production.
- **Teardown-before-reclaim barrier**: on `Disconnect{node}`, transitions peer's QP to ERROR state before emitting `DisconnectAck`. Only after ack does `remote-lookup` reclaim landing slots.
- **Single pool-wide registration**: entire memory-tier pool registered once with `REMOTE_WRITE` access; resulting `rkey` shared via `local_region()`.
- **Connection table**: tracks identified (by PeerId from `private_data`) and unidentified connections. States: Active → Draining → Dead.
- **Epoll multiplexing**: CM event channel + command eventfd + stop eventfd for responsive shutdown.
- **Ephemeral ports**: binds port 0; supports multiple co-resident instances per NIC.
