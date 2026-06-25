# Contract: IDispatcher (dispatcher-p2p implementation)

The dispatcher-p2p component implements the `IDispatcher` interface defined in `components/interfaces/src/idispatcher.rs`. It is a drop-in replacement for the standard dispatcher, selected via YAML profile.

## Interface Methods

All methods have identical signatures and semantics to the standard dispatcher. The P2P path only affects the internal implementation of cold lookups (when `IDispatchMap::lookup()` returns `BlockDevice`).

### Cold-Path Affected Methods

| Method | P2P Behavior |
|--------|-------------|
| `lookup(key, ipc_handle)` | Cold: NVMe → P2P ring slot → D2D → client GPU. Then promote to memory-tier. |
| `lookup_async(key, ipc_handle)` | Same as lookup but returns GpuStream for caller to sync. |
| `batch_lookup(entries)` | Concurrent lookups; cold entries use P2P pipeline with thread partitioning. |

### Unchanged Methods

All other IDispatcher methods behave identically to the standard dispatcher:
`check`, `remove`, `populate`, `prepare_store`, `commit_store`, `cancel_store`, `touch`, `clear_memory_tier`, `flush_to_ssd`, `initialize`, `shutdown`.

## Initialization Contract

`initialize(config: DispatcherConfig)`:
1. Set up data drives (block devices + extent managers)
2. Initialize dispatch map, memory tier, background writer/evictor
3. Attempt P2P ring allocation (`P2pRing::new()`)
   - Success: cold lookups use P2P path
   - Failure: log reason, cold lookups use DRAM fallback
4. Path decision stored immutably for component lifetime

## Shutdown Contract

`shutdown()`:
1. Stop background writer and evictor
2. Drain pending writes
3. Release P2P ring (if allocated): free GPU memory, unmap BAR1, unregister SPDK DMA, destroy CUDA streams
4. Disconnect block devices

## Error Contract

All errors use `DispatcherError` enum. P2P-specific failures:
- NVMe read failure during P2P pipeline → `DispatcherError::IoError`
- D2D copy failure → `DispatcherError::IoError`
- Ring slot exhaustion is handled internally (caller blocks until slot available)
- P2P initialization failure is NOT an error to callers — transparent fallback to DRAM

## Selection

Configured via YAML profile (`full-p2p.yaml` → `crate: dispatcher-p2p`). The component is loaded by the server-yaml composition framework as a drop-in replacement.
