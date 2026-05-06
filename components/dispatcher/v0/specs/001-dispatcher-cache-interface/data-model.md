# Data Model: Dispatcher Cache Interface

**Date**: 2026-04-28 | **Plan**: [plan.md](plan.md)

## Entities

### CacheKey

- **Type**: `u64` (re-exported from `idispatch_map::CacheKey`)
- **Identity**: Unique per dispatch map instance
- **Lifecycle**: Created at populate time, removed at remove time or on background write failure

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
  - `metadata_pci_addr: String` — PCI BDF address of the metadata block device
  - `data_pci_addrs: Vec<String>` — PCI BDF addresses of N data block devices
  - `block_device_version: BlockDeviceVersion` — which block device version to use (default V2)
  - `extent_manager_version: ExtentManagerVersion` — which extent manager version to use (default V2)
  - `max_cache_entries: usize` — maximum entries before eviction (default 10000; 0 disables eviction)
  - `eviction_threshold: f64` — fraction of max at which eviction triggers (default 0.8)
  - `format_on_init: bool` — whether to format extent managers on init (default true)
- **Constraints**: `data_pci_addrs` must be non-empty; each address must be unique
- **Relationships**: N = `data_pci_addrs.len()` determines the number of data devices and extent managers

### BackgroundWriteJob

- **Type**: Internal struct (not in interfaces)
- **Fields**:
  - `key: CacheKey` — cache entry to write
  - `size: u32` — data size in bytes
  - `device_index: usize` — which data device to target
- **Lifecycle**: Created by populate after staging copy completes; consumed by background writer; discarded on success or failure

### PendingWrite

- **Type**: Internal struct (not in interfaces)
- **Fields**:
  - `write_handle: WriteHandle` — extent reservation (publish commits, drop aborts)
  - `buffer: Arc<DmaBuffer>` — DMA buffer the caller writes into
  - `size: u32` — original data size in bytes
  - `drive_idx: usize` — index into data_drives for the target SSD
- **Lifecycle**: Created by `prepare_store`; consumed by `commit_store` (writes to SSD) or `cancel_store` (drops handle, aborting reservation)

## State Transitions

### Cache Entry Lifecycle

```
                             populate()                    background write
[Not Exists] ─────────────────────────> [Staging] ──────────────────────> [BlockDevice]
     ^                                      |                                  |
     |                                      +── write failure ──> [Not Exists] |
     |                                      |                                  |
     |                                      +── remove() ───────> [Not Exists] |
     |                                                                         |
     +─────────────────────── remove() ────────────────────────────────────────+

                          prepare_store()               commit_store()
[Not Exists] ─────────────────────────> [Pending] ──────────────────────> [BlockDevice]
                                            |
                                            +── cancel_store() ──> [Not Exists]
```

### Dispatcher Lifecycle

```
[Created] --bind receptacles--> [Configured] --initialize()--> [Operational] --shutdown()--> [Stopped]
                                                                     |
                                                               [serve lookup/check/remove/populate/
                                                                prepare_store/commit_store/cancel_store/touch]
```

## Relationships

```
Dispatcher 1 --- 1 ILogger (receptacle, optional)
Dispatcher 1 --- 1 IDispatchMap (receptacle, required)
Dispatcher 1 --- 1 Metadata BlockDevice (created during init)
Dispatcher 1 --- N Data BlockDevices (created during init)
Dispatcher 1 --- N ExtentManagers (created during init)
Data BlockDevice[i] 1 --- 1 ExtentManager[i]
ExtentManager[i] --- 1 Metadata BlockDevice (namespace partition i)
```
