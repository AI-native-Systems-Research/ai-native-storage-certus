# dispatcher (v1)

**Crate**: `dispatcher-v1`
**Path**: `components/dispatcher/v1/`
**Version**: 1.0.0

## Description

Memory-tier dispatcher component that manages the GPU-to-DRAM-to-SSD storage pipeline. Orchestrates a DRAM memory pool (via `IMemoryTier`), block devices, extent managers, and the dispatch map to provide a high-level cache API with LRU eviction and write-through persistence.

On `populate`, copies GPU data via DMA into a memory-tier slot, then queues an asynchronous write-through to SSD. When the memory pool is full, evicts LRU entries whose write-through has completed. On `lookup`, serves from memory-tier (fast path), promotes from SSD back to memory-tier (with pipelined read), or falls back to legacy staging buffers.

On `initialize`, creates and initializes N data block devices and N extent managers from provided PCI addresses, and starts a background write-through worker. On `shutdown`, completes all in-flight writes then tears down managed subsystems.

## Component Definition

```
DispatcherComponentV0 {
    version: "0.1.0",
    provides: [IDispatcher],
    receptacles: {
        logger: ILogger,
        dispatch_map: IDispatchMap,
        gpu_services: IGpuServices,
        spdk_env: ISPDKEnv,
        memory_tier: IMemoryTier,
    },
}
```

## Interfaces Provided

| Interface | Key Methods |
|-----------|------------|
| `IDispatcher` | `initialize(config) -> Result<(), DispatcherError>` -- configure PCI devices, start subsystems and background writer |
|              | `shutdown() -> Result<(), DispatcherError>` -- drain background writes, orderly teardown |
|              | `populate(key, ipc_handle) -> Result<(), DispatcherError>` -- DMA-copy from GPU to memory-tier slot, evict LRU if full, queue write-through to SSD |
|              | `lookup(key, ipc_handle) -> Result<(), DispatcherError>` -- serve from memory-tier (fast), promote from SSD, or read from staging |
|              | `check(key) -> Result<bool, DispatcherError>` -- test existence without transfer |
|              | `remove(key) -> Result<(), DispatcherError>` -- free memory-tier slot and/or SSD extent |

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging |
| `dispatch_map` | `IDispatchMap` | Yes | Extent-to-location tracking and reference counting |
| `gpu_services` | `IGpuServices` | Yes | GPU DMA copy operations (`dma_copy_to_host`, `dma_copy_to_device`) |
| `spdk_env` | `ISPDKEnv` | No | SPDK environment; if unconnected, operates in staging-only mode |
| `memory_tier` | `IMemoryTier` | Yes | DRAM pool for caching data between GPU and SSD |

## Key Types

- `DispatcherConfig { metadata_pci_addr, data_pci_addrs }` -- initialization configuration
- `IpcHandle { address: *mut u8, size: u32 }` -- opaque GPU memory pointer for DMA transfers
- `DispatcherError` -- `NotInitialized`, `KeyNotFound`, `AlreadyExists`, `AllocationFailed`, `IoError`, `Timeout`, `InvalidParameter`

## Internal Modules

- `background` -- async memory-tier-to-SSD write-through worker thread
- `io_segmenter` -- splits large DMA transfers into block-device-aligned segments (128 KiB default)
- `pipeline` -- pipelined SSD-to-GPU reads with ring-buffer reader for lookup promotion

## Key Differences from v0

- Memory-tier pool replaces staging buffers as primary data landing zone
- Capacity-based LRU eviction (via `IMemoryTier::evict_lru`) replaces count-based TSC eviction
- Lookup promotes SSD entries back to memory-tier for subsequent fast access
- Write-through is best-effort; populate succeeds even if SSD write is deferred
