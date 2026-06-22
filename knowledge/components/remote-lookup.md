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
    receptacles: { logger: ILogger },
}
```

## Interfaces Provided

| Interface | Methods |
|-----------|---------|
| `IRemoteLookup` | `batch_lookup(&[(CacheKey, IpcHandle)]) -> Vec<Result<(), RemoteLookupError>>` |
|                  | `join_cluster(endpoint: &str) -> Result<(), RemoteLookupError>` |
|                  | `leave_cluster() -> Result<(), RemoteLookupError>` |

## Receptacles

| Name | Interface | Required |
|------|-----------|----------|
| `logger` | `ILogger` | No (gracefully skips if unbound) |

## Key Design Decisions

- **Same types as IDispatcher**: `batch_lookup` uses `&[(CacheKey, IpcHandle)]` — identical to the dispatcher's batch_lookup — enabling zero-cost delegation.
- **Graceful degradation**: If the receptacle isn't bound in the dispatcher, the remote lookup phase is skipped and `KeyNotFound` is returned directly.
- **Placeholder**: All methods log and return stub results. The interface contract is established for future network implementation.

## Integration Points

The dispatcher calls this component in three places:
1. `initialize()` → `join_cluster("certus://local-cluster")`
2. `shutdown()` → `leave_cluster()`
3. `batch_lookup()` → forwards `KeyNotFound` entries after exhausting local tiers

## Wired By

- `certus-server-yaml` profiles: wired as `dispatcher.remote_lookup` → `remote_lookup` component
- `certus-server`: manual wiring in `main.rs`
