# extent-manager (v2)

**Crate**: `extent-manager-v2`
**Path**: `components/extent-manager/v2/`
**Version**: 0.1.0 (interface version `"0.3.0"`)
**Features**: `spdk` (default), `testing`

## Description

Fixed-size extent allocator with crash-consistent on-disk layout. Manages a data disk and a separate metadata disk. Supports two-phase extent reservation (allocate slot, get `WriteHandle`, publish or abort), lookup, removal, iteration, and periodic checkpointing for crash recovery.

### On-Disk Layout

- **Metadata device**: superblock + two checkpoint regions (for atomic swap)
- **Data device**: buddy allocator + slab allocator per region

### Crash Recovery

On `initialize`, reads the superblock and latest valid checkpoint from the metadata device, then reconstructs in-memory allocator state. Checkpoints are coalesced and can be triggered explicitly or by a background thread (default: every 5 seconds).

## Component Definition

```
ExtentManagerV2 {
    version: "0.3.0",
    provides: [IExtentManager],
    receptacles: {
        block_device: IBlockDevice,
        metadata_device: IBlockDevice,
        logger: ILogger,
    },
    fields: {
        regions: RwLock<Option<Vec<Arc<RwLock<RegionState>>>>>,
        shared: Mutex<Option<SharedState>>,
        checkpoint_coalesce: Mutex<CheckpointCoalesce>,
        checkpoint_done: Condvar,
        dma_alloc: Mutex<Option<DmaAllocFn>>,
        checkpoint_interval_ms: AtomicU64,
        shutdown: Arc<AtomicBool>,
        checkpoint_thread: Mutex<Option<JoinHandle<()>>>,
    },
}
```

## Interfaces Provided

| Interface | Key Methods |
|-----------|------------|
| `IExtentManager` | `format(params)` -- write superblock, initialize on-disk structures |
|                  | `initialize()` -- recover state from disk |
|                  | `reserve_extent(key, size) -> Result<WriteHandle, _>` -- two-phase allocate |
|                  | `lookup_extent(key) -> Result<Extent, _>` |
|                  | `get_extents() -> Vec<Extent>` |
|                  | `for_each_extent(callback)` |
|                  | `remove_extent(key)` |
|                  | `checkpoint()` -- persist in-memory state to metadata device |
|                  | `get_instance_id() -> Result<u64, _>` |

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `block_device` | `IBlockDevice` | Yes | Data device for extent storage |
| `metadata_device` | `IBlockDevice` | Yes | Metadata device for superblock and checkpoints |
| `logger` | `ILogger` | No | Optional logging |

## Key Types

- `ExtentKey = u64` -- opaque extent identifier
- `Extent { key, size, offset }` -- a committed storage extent
- `FormatParams { data_disk_size, slab_size, max_extent_size, sector_size, region_count, metadata_alignment, instance_id: Option<u64>, metadata_disk_ns_id }` -- format configuration
- `FormatParams::new(data_disk_size, instance_id: Option<u64>)` -- constructor with defaults
- `WriteHandle` -- two-phase commit handle: `publish()` commits, `abort()` or drop rolls back
- `ExtentManagerError` -- `CorruptMetadata`, `DuplicateKey`, `IoError`, `KeyNotFound`, `NotInitialized`, `OutOfSpace`

## Internal Modules

- `bitmap` -- bit-level allocation tracking
- `block_io` -- `BlockDeviceClient` wrapper for sync I/O
- `buddy` -- `BuddyAllocator` for large allocations
- `checkpoint` -- checkpoint serialization and atomic swap
- `recovery` -- state reconstruction from checkpoint
- `region` -- `RegionState`, `SharedState`
- `slab` -- `Slab`, `SizeClassManager` for small allocations
- `superblock` -- `Superblock` format and validation
- `write_handle` -- two-phase commit handle
