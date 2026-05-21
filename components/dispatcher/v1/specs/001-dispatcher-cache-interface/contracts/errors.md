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
| `KeyNotFound` | `lookup`, `remove`, `check`, `commit_store`, `cancel_store`, `touch` | Key does not exist in dispatch map or no pending write |
| `AlreadyExists` | `populate`, `prepare_store` | Key already exists in dispatch map |
| `AllocationFailed` | `populate`, `prepare_store` | DMA staging buffer or extent allocation fails |
| `IoError` | `lookup`, `initialize`, `commit_store` | Block device read/write error, device init failure |
| `Timeout` | `lookup`, `remove` | Dispatch map blocking operation exceeds timeout |
| `InvalidParameter` | `initialize`, `populate`, `prepare_store` | Zero-size IPC handle/size, empty PCI address list |

## Trait Implementations

- `fmt::Display` — human-readable messages for each variant
- `std::error::Error` — standard error trait
