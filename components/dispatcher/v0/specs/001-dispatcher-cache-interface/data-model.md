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

- **Type**: New struct in interfaces crate
- **Fields**:
  - `metadata_pci_addr: PciAddress` — PCI address of the metadata block device
  - `data_pci_addrs: Vec<PciAddress>` — PCI addresses of N data block devices
- **Constraints**: `data_pci_addrs` must be non-empty; each address must be unique
- **Relationships**: N = `data_pci_addrs.len()` determines the number of data devices and extent managers

### BackgroundWriteJob

- **Type**: Internal struct (not in interfaces)
- **Fields**:
  - `key: CacheKey` — cache entry to write
  - `buffer: Arc<DmaBuffer>` — staging buffer contents
  - `size: u32` — data size in bytes
  - `device_index: usize` — which data device to target
- **Lifecycle**: Created by populate after staging copy completes; consumed by background writer; discarded on success or failure

## State Transitions

### Cache Entry Lifecycle

```
[Not Exists] --populate()--> [Staging] --background write--> [BlockDevice] --remove()--> [Not Exists]
                                |                                  |
                                +--write failure--> [Not Exists]   |
                                |                                  |
                                +--remove()--> [Not Exists]        +--remove()--> [Not Exists]
```

### Dispatcher Lifecycle

```
[Created] --bind receptacles--> [Configured] --initialize()--> [Operational] --shutdown()--> [Stopped]
                                                                     |
                                                               [serve lookup/check/remove/populate]
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
