# Data Model: Dispatch Map Component

**Date**: 2026-04-27

## Entities

### CacheKey

Type alias for `u64`. Uniquely identifies an extent in the dispatch map. Matches `ExtentKey` from the extent manager.

### Location

Enum representing where extent data resides.

| Variant | Fields | Size | Description |
|---------|--------|------|-------------|
| Staging | buffer: `Arc<DmaBuffer>` | 8 bytes | Shared reference to DMA staging buffer |
| BlockDevice | offset: `u64` | 8 bytes | Byte offset on the block device |

Transitions: Staging → BlockDevice (one-way via `convert_to_storage`). No reverse transition.

### DispatchEntry

Per-key metadata stored in the hash map.

| Field | Type | Size | Description |
|-------|------|------|-------------|
| location | Location | 16 bytes | Where the data resides (enum with Arc or u64) |
| size_blocks | u32 | 4 bytes | Extent size in 4KiB blocks |
| read_ref | u32 | 4 bytes | Active reader count |
| write_ref | u32 | 4 bytes | Active writer count (0 or 1) |
| tsc | u64 | 8 bytes | RDTSC timestamp — set on creation, updated on lookup/touch |
| **Total** | | **~40 bytes** | + padding |

The `tsc` field enables LRU-style eviction: `oldest_keys()` sorts entries by ascending TSC, and `touch()`/`lookup()` refresh it via `rdtsc()`.

### DispatchMapState

Internal synchronization wrapper (not exposed via interface).

| Field | Type | Description |
|-------|------|-------------|
| inner | `Mutex<DispatchMapInner>` | Protected struct containing the entries HashMap |
| condvar | `Condvar` | Wakes threads blocked on ref count conditions |
| dma_alloc | `Mutex<Option<DmaAllocFn>>` | DMA buffer allocator, set via `set_dma_alloc` |

The implementation uses a single Mutex protecting both the entries map and the staging buffers (buffers are stored inside the `Location::Staging` variant as `Arc<DmaBuffer>`).

### LookupResult

Return type for `lookup()`.

| Variant | Fields | Description |
|---------|--------|-------------|
| NotExist | — | Key not found in map |
| MismatchSize | — | Key found but caller-expected size differs |
| Staging | buffer: `Arc<DmaBuffer>` | Shared reference to DMA staging buffer |
| BlockDevice | offset: `u64` | Byte offset on the block device |

### DispatchMapError

Error enum for all IDispatchMap operations.

| Variant | Fields | When |
|---------|--------|------|
| KeyNotFound | key: CacheKey | Operation on non-existent key |
| AlreadyExists | key: CacheKey | `create_staging` on key that already exists |
| ActiveReferences | key: CacheKey | `remove` while refs > 0 |
| Timeout | key: CacheKey | Blocking wait exceeded deadline |
| AllocationFailed | msg: String | DMA buffer allocation failed |
| InvalidSize | — | `create_staging` with size=0 |
| NotInitialized | msg: String | Operation before `initialize()` or missing DmaAllocFn |
| RefCountUnderflow | key: CacheKey | `release_read`/`release_write` when count is 0 |
| NoWriteReference | key: CacheKey | `downgrade_reference` without write ref held |
| InvalidState | msg: String | `convert_to_storage` on non-staging entry |

## State Machine

```
                    create_staging(key, size)
                           │
                           ▼
    ┌─────────────────────────────────────┐
    │            Staging                   │
    │  ptr, len, extent_manager_id, size  │
    │  write_ref=1                        │
    └───────────────┬─────────────────────┘
                    │ convert_to_storage(key, offset, device_id)
                    ▼
    ┌─────────────────────────────────────┐
    │          BlockDevice                 │
    │  offset, device_id, size            │
    │  write_ref=0, read_ref=0            │
    └───────────────┬─────────────────────┘
                    │ remove(key)  [requires all refs=0]
                    ▼
              (entry deleted)
```

Recovery path: `initialize()` → entries created directly as BlockDevice (from extent manager iteration).

## Relationships

```
DispatchMapComponent
    ├── provides: IDispatchMap
    ├── receptacle: ILogger (logging)
    ├── receptacle: IExtentManager (recovery via for_each_extent)
    └── internal: DispatchMapState
                    ├── inner: Mutex<{ entries: HashMap<CacheKey, DispatchEntry> }>
                    ├── condvar: Condvar
                    └── dma_alloc: Mutex<Option<DmaAllocFn>>
```
