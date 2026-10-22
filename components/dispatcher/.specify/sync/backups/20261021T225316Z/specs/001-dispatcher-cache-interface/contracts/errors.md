# Error Contract: DispatcherError

**Crate**: `interfaces` | **Feature gate**: none (available without `spdk`)

## Definition

```rust
/// Errors returned by `IDispatcher` operations.
#[derive(Debug, Clone)]
pub enum DispatcherError {
    /// Component not initialized or missing required receptacles.
    NotInitialized(String),
    /// The specified cache key was not found.
    KeyNotFound(CacheKey),
    /// A cache entry with this key already exists.
    AlreadyExists(CacheKey),
    /// DMA buffer allocation failed (out of memory).
    AllocationFailed(String),
    /// Block device or extent manager I/O error.
    IoError(String),
    /// A blocking operation exceeded the 100ms timeout.
    Timeout(String),
    /// Invalid parameter (e.g., zero-size IPC handle, empty config).
    InvalidParameter(String),
}
```

## Variant Usage

| Variant | Raised by | Condition |
|---------|-----------|-----------|
| `NotInitialized` | all methods except `shutdown` | Receptacles not bound or `initialize()` not called |
| `KeyNotFound` | `lookup`, `batch_lookup`, `remove`, `check`, `commit_store`, `cancel_store`, `touch` | Key does not exist in dispatch map or no pending write. **Also a remote lookup that returned `RemoteLookupError::NotFound`** — no peer holds it either, so the key is absent everywhere, which is a miss *(Sync 2026-09-28)*. **For `remove`, also** an entry held by a reader's or client's pin, or one still write-locked when the dispatch-map wait times out; in both cases nothing is freed. The pin of the key's own pending write-through does not refuse `remove` (001 FR-018) *(Sync 2026-10-14, branch `fix/dispatcher-remove-clear-mt`)* |
| `AlreadyExists` | `populate`, `prepare_store` | Key already exists in dispatch map |
| `AllocationFailed` | `populate`, `prepare_store` | DMA staging buffer or extent allocation fails |
| `IoError` | `lookup`, `batch_lookup`, `initialize`, `commit_store` | Block device read/write error, device init failure. From the remote path, **only** `RemoteLookupError::TransportError` — a fetch that failed, not a key that was absent *(Sync 2026-09-28: previously a remote `NotFound` also arrived here, which made it uncountable as a miss)* |
| `Timeout` | `pin`, `clear_memory_tier` | `pin`: the dispatch-map wait for a writer exceeded its timeout. `clear_memory_tier`: the tier could not be emptied within 5 s because entries stayed pinned, and those entries remain resident *(Sync 2026-10-14, branch `fix/dispatcher-remove-clear-mt`: `lookup` and `remove` never return `Timeout`; they report a dispatch-map timeout as `KeyNotFound`)* |
| `InvalidParameter` | `initialize`, `populate`, `prepare_store` | Zero-size IPC handle/size, empty PCI address list |

## Trait Implementations

- `fmt::Display` — human-readable messages for each variant
- `std::error::Error` — standard error trait
