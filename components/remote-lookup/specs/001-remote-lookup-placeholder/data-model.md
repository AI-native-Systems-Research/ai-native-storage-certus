# Data Model: Remote Lookup Batch Interface

## Entities

### CacheKey

- **Type**: `u64`
- **Source**: `components/interfaces/src/idispatch_map.rs`
- **Description**: Opaque integer identifier for a cached data entry. Shared across `IDispatcher` and `IRemoteLookup` interfaces.
- **Validation**: Any valid `u64` value.

### IpcHandle

- **Type**: Struct with `address: *mut u8` and `size: u32`
- **Source**: `components/interfaces/src/idispatcher.rs`
- **Description**: Opaque handle to client GPU memory for DMA transfers. In the placeholder implementation, the handle is not dereferenced — only its `size` field is read for logging.
- **Validation**: `size` must be > 0 for meaningful entries; the placeholder does not enforce this.
- **Safety**: Implements `Send` (GPU memory accessible cross-thread via DMA).

### RemoteLookupError

- **Type**: Enum
- **Source**: `components/interfaces/src/iremote_lookup.rs`
- **Variants**:
  - `NotConnected` — connection not established
  - `NotFound` — key not found (placeholder always returns this when connected)
  - `TransportError(String)` — network/transport failure (unused in placeholder)
- **Implements**: `Display`, `Error`, `Debug`, `Clone`, `PartialEq`, `Eq`

## Relationships

```text
IRemoteLookup::batch_lookup
    input:  &[(CacheKey, IpcHandle)]
    output: Vec<Result<(), RemoteLookupError>>

    CacheKey ──── identifies ───→ cached entry (on remote node)
    IpcHandle ─── describes ────→ destination GPU memory (unused in placeholder)
```

## State

The component has a single boolean state: `connected` (AtomicBool).

- `connected = false`: All operations return `NotConnected`
- `connected = true`: Placeholder logs and returns `NotFound` per entry
