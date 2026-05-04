# Dispatcher Component v0 — Design Document

## Overview

The dispatcher orchestrates GPU-to-SSD cache operations for the Certus storage system. It manages N data NVMe drives with N extent managers, providing a unified key-value interface for GPU memory caching with persistent SSD backing.

## Component Architecture

```
                        +-----------------------+
                        | DispatcherComponentV0 |
                        +-----------+-----------+
                                    |
            provides: IDispatcher   |   receptacles:
                                    |
         +----------+----------+----+----+-----------+
         |          |          |         |           |
     ILogger   IDispatchMap  IGpuServices  ISPDKEnv
                                    |
                    +---------------+---------------+
                    |               |               |
              DataDrive[0]    DataDrive[1]    DataDrive[N-1]
              (BlockDev +     (BlockDev +     (BlockDev +
               ExtentMgr)     ExtentMgr)      ExtentMgr)
```

### Internal State

| Field | Type | Purpose |
|-------|------|---------|
| `initialized` | `AtomicBool` | Guards all operations; set after `initialize()` |
| `data_drives` | `Mutex<Vec<DataDrive>>` | N block-device + extent-manager pairs |
| `bg_writer` | `Mutex<Option<BackgroundWriter>>` | Async staging-to-SSD writer thread |
| `pending_writes` | `Mutex<HashMap<CacheKey, PendingWrite>>` | In-flight prepare_store reservations |
| `eviction_watermark` | `AtomicUsize` | Cache entry count triggering eviction |

### DataDrive

Each data drive bundles:
- **Block device** (`IBlockDevice`) — NVMe I/O via SPDK userspace driver
- **Block device admin** (`IBlockDeviceAdmin`) — initialization/shutdown lifecycle
- **Extent manager** (`ExtentManagerV2`) — fixed-size extent allocator with crash-consistent metadata

## Data Flow

### Populate (GPU -> Staging -> SSD)

```
Client GPU Memory
       |
       | dma_copy_to_host (IGpuServices)
       v
  DMA Staging Buffer (dispatch map)
       |
       | BackgroundWriter thread (async)
       v
  Extent Manager: reserve_extent(key, size)
       |
       | write_buffer_to_ssd (segmented I/O)
       v
  NVMe SSD (block device)
       |
       | write_handle.publish()
       v
  Dispatch Map: convert_to_storage(key, offset)
```

1. `create_staging(key, blocks)` allocates a DMA buffer and registers the key
2. GPU data is DMA-copied into the staging buffer
3. Reference is downgraded (write -> read) and a `WriteJob` is enqueued
4. Background writer drains jobs: reserves extent, writes to SSD, publishes metadata

### Prepare/Commit Store (Direct SSD write, no staging)

```
Client
  |
  | prepare_store(key, size) -> Arc<DmaBuffer>
  v
DMA Buffer (caller writes directly)
  |
  | commit_store(key)
  v
Extent Manager: reserve already done
  |
  | write_buffer_to_ssd (segmented I/O)
  v
NVMe SSD
  |
  | write_handle.publish()
  v
Dispatch Map: convert_to_storage(key, offset)
```

- `prepare_store` registers the key, reserves an extent, and returns a DMA buffer
- The caller fills the buffer directly (no GPU DMA involved)
- `commit_store` writes the buffer to SSD and publishes the extent
- `cancel_store` drops the handle (auto-abort) and removes the dispatch map entry

### Lookup (SSD -> GPU)

```
NVMe SSD
  |
  | read_from_block_device (segmented I/O)
  v
DMA Read Buffer (reassembled)
  |
  | dma_copy_to_device (IGpuServices)
  v
Client GPU Memory
```

If the entry is still in staging, the staging buffer is copied directly to GPU without SSD I/O.

## Algorithms

### Drive Selection

Deterministic striping by cache key:

```
drive_index = key % num_drives
```

Both reads and writes use the same mapping, ensuring a key always targets the same drive.

### I/O Segmentation (MDTS)

NVMe devices impose a Maximum Data Transfer Size (MDTS), typically 128 KiB. The `io_segmenter` module splits large transfers into compliant segments:

```
segment_io(start_lba, total_bytes, max_transfer_size, sector_size) -> Vec<IoSegment>
```

Each segment carries:
- `buffer_offset` — byte offset into the source/destination buffer
- `lba` — starting logical block address for this segment
- `length` — bytes in this segment (<= MDTS)

Used by both `write_buffer_to_ssd` and `read_from_block_device`.

### Eviction

Capacity-based eviction runs before `prepare_store` and (implicitly) before `populate` via the background writer:

1. Query all keys from the dispatch map ordered oldest-first
2. If `count <= eviction_watermark`, return immediately
3. For each oldest key until count reaches watermark:
   - Acquire write lock (`take_write`)
   - Remove from dispatch map
   - If entry was on block device, free the extent (`remove_extent`)
   - Skip entries that are locked (in-flight I/O)

The watermark is computed as:
```
eviction_watermark = max_cache_entries * eviction_threshold
```

Default: 10,000 max entries, 80% threshold = eviction starts at 8,000 entries.

### Extent Allocation

Per-drive extent managers use a buddy allocator with crash-consistent on-disk metadata:

1. `reserve_extent(key, size)` -> `WriteHandle` (reservation, not yet visible)
2. Caller writes data to the extent's LBA range
3. `publish()` atomically commits the extent metadata (makes it visible)
4. On error or `cancel_store`, dropping the `WriteHandle` calls abort (frees the reservation)

Format parameters are derived from actual disk geometry:
- `slab_size` = largest power-of-2 fitting in `region_size / 16`
- This allows ~16 slabs per region, supporting multiple size classes
- `max_extent_size` is clamped to `slab_size`

### Background Writer

A dedicated thread (`dispatcher-bg-writer`) drains a channel of `WriteJob` messages:

- Runs until shutdown flag is set AND channel is empty
- Each job: lookup staging buffer -> reserve extent -> segmented SSD write -> publish -> convert_to_storage
- On any failure, the write handle is dropped (auto-abort), and the entry remains in staging
- Shutdown blocks until all queued jobs are processed

## Lifecycle

```
new_default() -> bind receptacles -> initialize(config) -> [use IDispatcher] -> shutdown()
```

### Initialize

1. Validate config (non-empty PCI addresses)
2. Create N block devices via SPDK (probe NVMe controllers)
3. Create N extent managers, format with disk-derived parameters
4. Start background writer thread
5. Compute eviction watermark from config
6. Set `initialized = true`

### Shutdown

1. Drain background writer (all queued jobs complete)
2. Clear pending writes (auto-abort any uncommitted prepare_store calls)
3. Shut down block devices in reverse order
4. Set `initialized = false`

## Error Handling

All `IDispatcher` methods return `Result<T, DispatcherError>`. Error variants:

| Variant | Cause |
|---------|-------|
| `NotInitialized` | Method called before `initialize()` or after `shutdown()` |
| `KeyNotFound` | Lookup/remove/commit on a key that doesn't exist |
| `AlreadyExists` | Populate/prepare_store with a duplicate key |
| `AllocationFailed` | DMA buffer or extent allocation failure (OOM) |
| `IoError` | Block device I/O failure, channel disconnect |
| `Timeout` | (Reserved) blocking operation exceeded deadline |
| `InvalidParameter` | Zero-size handle, empty config, malformed PCI address |

## Concurrency

- All public methods are `&self` (no exclusive access required)
- `data_drives` lock is held briefly (index lookup), then dropped before I/O
- `pending_writes` lock is held only for insert/remove operations
- Background writer runs on a dedicated OS thread with channel-based communication
- Dispatch map provides read/write reference counting for concurrent access
- Eviction skips locked entries rather than blocking
