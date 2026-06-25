# remote-lookup

## Summary

Remote lookup placeholder component for the Certus storage system. Provides the `IRemoteLookup` interface for performing cache lookups to other Certus nodes on the network. Currently a stub implementation that logs requests and returns `NotFound` — the network transport will be implemented in a future iteration.

`RemoteLookupComponent` implements `IRemoteLookup` and declares a receptacle for `ILogger`. The dispatcher forwards any cache entries not found locally (after exhausting memory-tier and SSD) to this interface for remote resolution.

## Architecture

### Integration with Dispatcher

The dispatcher calls `IRemoteLookup` in three places:

1. **`initialize()`** — calls `join_cluster("certus://local-cluster")`
2. **`shutdown()`** — calls `leave_cluster()`
3. **`batch_lookup()`** — after local lookup (memory-tier + SSD cold path), any `KeyNotFound` entries are forwarded to `IRemoteLookup::batch_lookup` for remote resolution

### Current Behavior (Placeholder)

All methods log via `ILogger` and return stub results:
- `batch_lookup` → `Err(RemoteLookupError::NotFound)` for each entry
- `join_cluster` → `Ok(())` (logs endpoint)
- `leave_cluster` → `Ok(())` (logs call)

## Interface

| Method | Description |
|--------|-------------|
| `batch_lookup(&[(CacheKey, IpcHandle)]) -> Vec<Result<(), RemoteLookupError>>` | Look up multiple keys on remote nodes |
| `join_cluster(endpoint: &str) -> Result<(), RemoteLookupError>` | Join a cluster of Certus nodes |
| `leave_cluster() -> Result<(), RemoteLookupError>` | Leave the cluster |

## Receptacles

| Name | Interface | Purpose |
|------|-----------|---------|
| `logger` | `ILogger` | Diagnostic logging |

## Usage

```rust
use component_core::query_interface;
use interfaces::{CacheKey, IpcHandle, IRemoteLookup, RemoteLookupError};
use remote_lookup::RemoteLookupComponent;

let comp = RemoteLookupComponent::new();
let rl = query_interface!(comp, IRemoteLookup).unwrap();

rl.join_cluster("192.168.1.10:9090").unwrap();

let mut buf = vec![0u8; 4096];
let entries = vec![(1u64, IpcHandle { address: buf.as_mut_ptr(), size: 4096 })];
let results = rl.batch_lookup(&entries);
// Placeholder returns NotFound for each entry
assert_eq!(results[0], Err(RemoteLookupError::NotFound));
```

## Build & Test

```bash
cargo build -p remote-lookup
cargo test -p remote-lookup
cargo clippy -p remote-lookup -- -D warnings
cargo doc -p remote-lookup --no-deps
```
