# dispatcher (v0)

**Crate**: `dispatcher`
**Path**: `components/dispatcher/v0/`
**Version**: 0.1.0

## Description

Orchestrator component that manages the full storage pipeline for GPU memory caching. Coordinates block devices, extent managers, and the dispatch map to provide a high-level cache API (populate/lookup/remove) with asynchronous staging-to-SSD writeback.

On `initialize`, creates and initializes N data block devices and N extent managers from provided PCI addresses. On `shutdown`, completes all in-flight background writes then tears down managed subsystems. Background writes are queued via an I/O segmenter that handles large transfers by splitting them into block-device-aligned segments.

## Component Definition

```
DispatcherComponentV0 {
    version: "0.1.0",
    provides: [IDispatcher],
    receptacles: {
        logger: ILogger,
        block_device_admin: IBlockDeviceAdmin,
        dispatch_map: IDispatchMap,
    },
}
```

## Interfaces Provided

| Interface | Key Methods |
|-----------|------------|
| `IDispatcher` | `initialize(config) -> Result<(), DispatcherError>` -- configure PCI devices, start subsystems |
|              | `shutdown() -> Result<(), DispatcherError>` -- drain background writes, orderly teardown |
|              | `populate(key, ipc_handle) -> Result<(), DispatcherError>` -- DMA-copy from GPU, stage, async write to SSD |
|              | `lookup(key, ipc_handle) -> Result<(), DispatcherError>` -- find data (staging or SSD), DMA-copy to GPU |
|              | `check(key) -> Result<bool, DispatcherError>` -- test existence without transfer |
|              | `remove(key) -> Result<(), DispatcherError>` -- free staging/SSD resources |

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging |
| `block_device_admin` | `IBlockDeviceAdmin` | Yes | Block device lifecycle management |
| `dispatch_map` | `IDispatchMap` | Yes | Extent-to-location dispatch and staging buffer management |

## Key Types

- `DispatcherConfig { metadata_pci_addr, data_pci_addrs }` -- initialization configuration
- `IpcHandle { address: *mut u8, size: u32 }` -- opaque GPU memory pointer for DMA transfers
- `DispatcherError` -- `NotInitialized`, `KeyNotFound`, `AlreadyExists`, `AllocationFailed`, `IoError`, `Timeout`, `InvalidParameter`

## Internal Modules

- `background` -- async staging-to-SSD write queue
- `io_segmenter` -- splits large DMA transfers into block-device-aligned segments
