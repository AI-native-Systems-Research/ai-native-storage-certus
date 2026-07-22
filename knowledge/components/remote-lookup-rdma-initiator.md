# remote-lookup-rdma-initiator

**Crate**: `remote-lookup-rdma-initiator`
**Path**: `components/remote-lookup-rdma-initiator/`
**Version**: 0.1.0

## Description

Handles incoming remote cache requests from other Certus nodes. Provides zero-copy lookup via the memory-tier pool — returns raw pointers that remain valid until explicitly released. Designed for RDMA Write workflows where the data is consumed via network DMA before the read reference is released.

Currently a placeholder — all operations return `NotInitialized`. The interface contract is established for the future RDMA implementation.

## Component Definition

```
RemoteLookupRdmaInitiatorComponent {
    version: "0.1.0",
    provides: [IRemoteLookupRdmaInitiator],
    receptacles: {
        logger: ILogger,
        dispatcher: IDispatcher,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IRemoteLookupRdmaInitiator {
        fn handle_lookup(&self, key: CacheKey) -> Result<LookupRef, RemoteLookupRdmaInitiatorError>;
        fn handle_check(&self, key: CacheKey) -> Result<bool, RemoteLookupRdmaInitiatorError>;
        fn handle_batch_lookup(&self, keys: &[CacheKey]) -> Vec<Result<LookupRef, RemoteLookupRdmaInitiatorError>>;
        fn release_lookup(&self, key: CacheKey);
    }
}
```

## Verified Properties

None. No formal verification model exists for this component.

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging |
| `dispatcher` | `IDispatcher` | Yes | Delegated cache operations (lookup, check) |

## Key Types

- `LookupRef { ptr: *const u8, size: u32, key: CacheKey }` — zero-copy reference to cached data in memory-tier pool
- `RemoteLookupRdmaInitiatorError` — `InvalidRequest`, `KeyNotFound`, `DispatchError`, `NotInitialized`

## Usage Protocol

1. `handle_lookup(key)` → returns `LookupRef` with a pinned read reference
2. Caller consumes data (e.g., RDMA Write from `ptr`, `size` bytes)
3. `release_lookup(key)` → releases the read reference, allowing eviction
