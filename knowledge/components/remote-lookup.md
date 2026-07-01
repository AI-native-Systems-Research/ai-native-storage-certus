# remote-lookup

**Crate**: `remote-lookup`
**Path**: `components/remote-lookup/`
**Version**: 0.1.0

## Description

Placeholder component for remote cache lookups to other Certus nodes on the network. The dispatcher forwards cache misses (entries not found in memory-tier or SSD) to this component's `batch_lookup` method. Currently a stub that logs each request and returns `NotFound` — real network transport will be added later.

## Component Definition

```
RemoteLookupComponent {
    version: "0.1.0",
    provides: [IRemoteLookup],
    receptacles: {
        logger: ILogger,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IRemoteLookup {
        fn batch_lookup(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), RemoteLookupError>>;
        fn join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError>;
        fn leave_cluster(&self) -> Result<(), RemoteLookupError>;
    }
}
```

## Verified Properties

None. No formal verification model exists for this component.

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging (gracefully skips if unbound) |

## Key Types

- `RemoteLookupError` — `NotFound`, `TransportError(String)`

## Key Design Decisions

- **Same types as IDispatcher**: `batch_lookup` uses `&[(CacheKey, IpcHandle)]` — identical to the dispatcher's batch_lookup — enabling zero-cost delegation.
- **Graceful degradation**: If the receptacle isn't bound in the dispatcher, the remote lookup phase is skipped and `KeyNotFound` is returned directly.
- **Placeholder**: All methods log and return stub results. The interface contract is established for future network implementation.
