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
| `NotInitialized` | `initialize`, `lookup`, `check`, `remove`, `populate` | Receptacles not bound or `initialize()` not called |
| `KeyNotFound` | `lookup`, `remove`, `check` | Key does not exist in dispatch map |
| `AlreadyExists` | `populate` | Key already exists in dispatch map |
| `AllocationFailed` | `populate` | DMA staging buffer allocation fails |
| `IoError` | `lookup`, `initialize` | Block device read/write error, device init failure |
| `Timeout` | `lookup`, `remove` | Dispatch map blocking operation exceeds 100ms |
| `InvalidParameter` | `initialize`, `populate` | Zero-size IPC handle, empty PCI address list |

## Trait Implementations

- `fmt::Display` — human-readable messages for each variant
- `std::error::Error` — standard error trait
