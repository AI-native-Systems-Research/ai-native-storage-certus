# Data Model: Dispatcher Cache Interface

**Date**: 2026-04-28 | **Plan**: [plan.md](plan.md)

## Entities

### CacheKey

- **Type**: `u64` (re-exported from `idispatch_map::CacheKey`)
- **Identity**: Unique per dispatch map instance
- **Lifecycle**: Registered by `copy_gpu_to_memory_completed` (every store path) or `recover_extent`; removed by `remove`, `clear_all`, the SSD evictor, or `clear_memory_tier` for an unpersisted entry. A failed background write does not remove it (FR-017). *(Sync 2026-10-31 — BACKFILL.)*

### IpcHandle

- **Type**: New struct in interfaces crate
- **Fields**:
  - `address: *mut u8` — GPU memory base address
  - `size: u32` — size in bytes (must be > 0, bounded by extent manager's max extent size)
- **Constraints**: Caller guarantees validity; dispatcher does not validate GPU memory accessibility
- **Safety**: Marked `Send` (GPU memory is accessible cross-thread via DMA engine)

### DispatcherConfig

- **Type**: Struct in interfaces crate
- **Fields**:
  - `data_pci_addrs: Vec<String>` — PCI BDF addresses of N data block devices (each holds its own metadata partition)
  - `max_cache_entries: usize` — default 10000
  - `format_on_init: bool` — whether to format extent managers on init (default true)
  - the SSD-eviction, memory-tier-eviction, poller-placement, store-backpressure, partition-size and cold-staging fields listed in spec FR-033 (`components/interfaces/src/idispatcher.rs:26-102`)
- *(Sync 2026-10-31 — BACKFILL: `metadata_pci_addr`, `block_device_version`, `extent_manager_version` and `eviction_threshold` no longer exist.)*
- **Constraints**: `data_pci_addrs` must be non-empty; each address must be unique
- **Relationships**: N = `data_pci_addrs.len()` determines the number of data devices and extent managers

### BackgroundWriteJob

- **Type**: `WriteJob` (`src/background.rs:16-23`, internal; not in interfaces)
- **Fields**:
  - `key: u64` — cache entry to write
  - `size: u32` — data size in bytes
  - `device_index: usize` — which data device to target. Chosen round-robin per committing thread by `next_store_drive` and recorded in the entry's SSD location by the write-through (spec FR-039 step (4)) *(Sync 2026-10-27, commit `4bb69289`; previously a splitmix64 hash of the key)*
- **Lifecycle**: Created by `copy_gpu_to_memory_completed` (the end of every store: `populate`, `batch_populate`, shm-queue CommitStore) once the entry is registered as MemoryTier, and by `schedule_write_through`; consumed by the per-drive background writer (`ParallelBackgroundWriter`), which writes the memory-tier slot to an extent and records it with `convert_to_storage`; discarded on success or failure (a failure is recorded for `flush_to_ssd`, FR-017). While queued or running, the job holds a write-through pin on the entry (`WriteThroughPins`, FR-018).

### Reserved store slot

- **Type**: a memory-tier slot (`IMemoryTier::insert`) with no dispatch-map entry yet; there is no separate struct. *(Sync 2026-10-31 — BACKFILL: this replaces the `PendingWrite` entity of the removed `prepare_store`/`commit_store`/`cancel_store` path; none of those exist in `src/` or in `IDispatcher`.)*
- **Lifecycle**: Created by `reserve_memory(key, size, session_id, deadline)` (`src/lib.rs:3708`); filled by `copy_gpu_to_memory_async` (`:3826`); turned into a MemoryTier entry by `copy_gpu_to_memory_completed` (`:3917`); freed by `release_memory` (`:4021`) or by any failure before registration (FR-003, FR-056).

## State Transitions

### Cache Entry Lifecycle

Dispatch-map states are `MemoryTier { pointer, size, ssd_offset }` and `BlockDevice { offset }` (`components/interfaces/src/idispatch_map.rs:10-27`; `ssd_offset` is internal to the dispatch map). `offset` / `ssd_offset` hold the encoded SSD location `(drive << 56) | extent_offset` (FR-039 step (4)).

```
 STORE
 [Not Exists]
   --reserve_memory--> [Reserved slot]
       (a memory-tier slot only; no dispatch-map entry yet)
 [Reserved slot]
   --copy_gpu_to_memory_async--> (slot filled)
   --release_memory, or a copy/sync failure--> [Not Exists]
 (slot filled)
   --copy_gpu_to_memory_completed--> [MemoryTier, ssd_offset = None]
       + WriteJob enqueued
 populate / batch_populate do reserve + copy + sync + completed in one call.

 WRITE-THROUGH
 [MemoryTier, ssd_offset = None]
   --background write lands, convert_to_storage-->
       [MemoryTier, ssd_offset = Some(loc)]
   --write fails--> unchanged (failure reported by flush_to_ssd)

 EVICTION (memory tier full, or the memory-tier evictor above its watermark)
 [MemoryTier, ssd_offset = Some(loc)]
   --try_evict_to_block (unpinned), slot freed--> [BlockDevice { loc }]
 [MemoryTier, ssd_offset = None]
   not evictable: skipped, never dropped
 [BlockDevice]
   --SSD evictor (SSD above threshold): dm.remove, extent freed-->
       [Not Exists]

 COLD PROMOTION (lookup, lookup_async, batch_lookup, promote_to_memory_tier)
 [BlockDevice { loc }]
   --slot reserved, SSD read, promote_block_to_memory_tier-->
       [MemoryTier, ssd_offset = Some(loc)]
   --tier full: staged serve (FR-053), or a read/delivery failure-->
       unchanged (slot released)

 REMOVE / CLEAR
 [MemoryTier | BlockDevice]
   --remove (no reader/client pin)--> [Not Exists]
       (slot and SSD extent freed)
 [MemoryTier]
   --clear_memory_tier--> [BlockDevice] if persisted, else [Not Exists]
 [any]
   --clear_all--> [Not Exists]

 RECOVERY (initialize with format_on_init = false)
 extent listed by drive i
   --recover_extent(key, encode_loc(i, offset), size)--> [BlockDevice]
```

Read pins (`take_read` / `release_read`, `pin` / `unpin`, a lookup's own pin, the write-through pin) block demotion, SSD eviction and `remove`; they do not change the state.

### Dispatcher Lifecycle

```
[Created] --bind receptacles--> [Configured] --initialize()--> [Operational]
[Operational] --shutdown()--> [Stopped]

[Operational] serves:
  lookup, lookup_async, batch_lookup, check, remove,
  populate, batch_populate,
  reserve_memory, copy_gpu_to_memory_async, copy_gpu_to_memory_completed,
  release_memory, pin, unpin, touch,
  promote_to_memory_tier, clear_memory_tier, clear_all,
  flush_to_ssd, schedule_write_through, read_to_bounce, release_bounce,
  read_write_stats, tier_event_stats
```

*(Sync 2026-10-31 — BACKFILL: the served list matches `IDispatcher`, `components/interfaces/src/idispatcher.rs`; the earlier list named `prepare_store` / `commit_store` / `cancel_store`, which no longer exist.)*

## Relationships

```
Dispatcher 1 --- 1 ILogger        (receptacle, optional)
Dispatcher 1 --- 1 IDispatchMap   (receptacle, required)
Dispatcher 1 --- 1 IGpuServices   (receptacle; required for every GPU copy)
Dispatcher 1 --- 1 ISPDKEnv       (receptacle; without it no drives are
                                   created: memory-tier-only mode)
Dispatcher 1 --- 1 IMemoryTier    (receptacle, required: the DRAM tier)
Dispatcher 1 --- 1 IRemoteLookup  (receptacle, optional: remote misses, FR-045)
Dispatcher 1 --- N DataDrives     (created during init, one per
                                   data_pci_addrs entry)
DataDrive[i] 1 --- 1 IBlockDevice
DataDrive[i] 1 --- 1 IPartitionTable (disk-partition-manager; partitions:
                                      metadata, extended metadata, data)
DataDrive[i] 1 --- 1 IExtentManager  (metadata in partition 0, extents in
                                      partition 2 of the same drive)
DataDrive[i] 1 --- 1 ChannelPool     (client channels for the read
                                      pipelines, FR-034)
Dispatcher 1 --- 1 ParallelBackgroundWriter (one writer per drive)
Dispatcher 1 --- 1 BackgroundEvictor (SSD) + 1 MemoryTierEvictor
Dispatcher 1 --- 0..1 PipelineRing (+ StagingPool, FR-053)
```

There is no separate metadata block device and no eviction-policy receptacle: each data drive holds its own metadata partition, and eviction order comes from the memory tier (`IMemoryTier::oldest_keys`) and the dispatch map. *(Sync 2026-10-31 — BACKFILL from `src/lib.rs:413-445` (receptacles and fields), `:285-293` (`DataDrive`), `:1913-1979` (partitions); the earlier list had a metadata block device, a namespace partition per extent manager, and no memory tier.)*
