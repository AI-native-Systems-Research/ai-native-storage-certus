# dispatch-map (v0)

**Crate**: `dispatch-map`
**Path**: `components/dispatch-map/v0/`
**Version**: 0.1.0

## Description

In-memory dispatch map that tracks where each extent's data currently lives -- either in a DMA staging buffer (awaiting writeback to the block device) or at a committed byte offset on the block device.

Implements readers-writer reference counting with blocking (`Condvar`) semantics:
- `take_read` blocks until no writer is active
- `take_write` blocks until no readers or writers are active
- 100ms timeout returns `DispatchMapError::Timeout`

On `initialize`, recovers all committed extents from the bound `IExtentManager` into the map.

## Component Definition

```
DispatchMapComponentV0 {
    version: "0.1.0",
    provides: [IDispatchMap],
    receptacles: { logger: ILogger, extent_manager: IExtentManager },
    fields: { state: DispatchMapState },
}
```

## Interfaces Provided

| Interface | Key Methods |
|-----------|------------|
| `IDispatchMap` | `set_dma_alloc(alloc)` -- set DMA allocator |
|               | `initialize()` -- recover committed extents from extent manager |
|               | `create_staging(key, size)` -- allocate DMA staging buffer for a key |
|               | `lookup(key) -> Result<LookupResult, _>` -- find where data lives |
|               | `convert_to_storage(key, offset)` -- promote staging to block device location |
|               | `take_read(key)` / `release_read(key)` -- reader reference counting |
|               | `take_write(key)` / `release_write(key)` -- writer reference counting |
|               | `downgrade_reference(key)` -- convert write lock to read lock |
|               | `remove(key)` -- remove entry from map |

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging |
| `extent_manager` | `IExtentManager` | Yes | Source of committed extents for recovery |

## Key Types

- `CacheKey = u64` -- extent identifier
- `LookupResult` -- `NotExist`, `MismatchSize`, `Staging { buffer: Arc<DmaBuffer> }`, `BlockDevice { offset: u64 }`
- `DispatchMapError` -- `KeyNotFound`, `AlreadyExists`, `ActiveReferences`, `Timeout`, `AllocationFailed`, `InvalidSize`, `NotInitialized`, `RefCountUnderflow`, `NoWriteReference`, `InvalidState`

## Internal Types

- `DispatchEntry { location, size_blocks, read_ref, write_ref }`
- `Location::Staging { buffer: Arc<DmaBuffer> }` -- data in DMA staging buffer
- `Location::BlockDevice { offset: u64 }` -- data committed to block device
