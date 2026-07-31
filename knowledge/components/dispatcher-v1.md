# dispatcher (v1)

**Crate**: `dispatcher`
**Path**: `components/dispatcher/`
**Version**: 1.0.0

## Description

Central data-plane cache orchestrator for the Certus storage pipeline. Manages a two-tier cache (DRAM + SSD) and routes GPU-to-DRAM-to-SSD data movement. Implements zero-copy cold-path reads where SPDK DMA writes directly into CUDA-pinned memory-tier buffers.

On `populate`, copies GPU data via DMA into a memory-tier slot, then queues asynchronous write-through to SSD. On `lookup`, serves from memory-tier (hot path), or promotes from SSD with pipelined NVMe reads directly into the memory-tier pool (cold path). On capacity exhaustion, evicts LRU entries whose write-through has completed.

On `initialize`, creates and initializes N data block devices and N extent managers from provided PCI addresses, manages GPT partition tables per drive, and starts background write-through workers and evictors. On `shutdown`, completes all in-flight writes then tears down managed subsystems.

## Component Definition

```
DispatcherComponent {
    version: "0.1.0",
    provides: [IDispatcher],
    receptacles: {
        logger: ILogger,
        dispatch_map: IDispatchMap,
        gpu_services: IGpuServices,
        spdk_env: ISPDKEnv,
        memory_tier: IMemoryTier,
        remote_lookup: IRemoteLookup,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IDispatcher {
        fn initialize(&self, config: DispatcherConfig) -> Result<(), DispatcherError>;
        fn shutdown(&self) -> Result<(), DispatcherError>;
        fn lookup(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>;
        fn lookup_async(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<GpuStream, DispatcherError>;
        fn batch_lookup(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>;
        fn check(&self, key: CacheKey) -> Result<bool, DispatcherError>;
        fn remove(&self, key: CacheKey) -> Result<(), DispatcherError>;
        fn populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>;
        fn reserve_memory(&self, key: CacheKey, size: u32, session_id: u64) -> Result<*mut u8, DispatcherError>;
        fn copy_gpu_to_memory_async(&self, key: CacheKey, ipc_handle: IpcHandle, stream: GpuStream) -> Result<(), DispatcherError>;
        fn copy_gpu_to_memory_completed(&self, key: CacheKey, size: u32) -> Result<(), DispatcherError>;
        fn release_memory(&self, key: CacheKey) -> Result<(), DispatcherError>;
        fn pin(&self, key: CacheKey) -> Result<(), DispatcherError>;
        fn unpin(&self, key: CacheKey) -> Result<(), DispatcherError>;
        fn touch(&self, key: CacheKey) -> Result<(), DispatcherError>;
        fn promote_to_memory_tier(&self, keys: &[CacheKey]);
        fn clear_memory_tier(&self) -> Result<usize, DispatcherError>;
        fn flush_to_ssd(&self) -> Result<usize, DispatcherError>;
        fn read_write_stats(&self) -> ReadWriteStats;
    }
}
```

## Verified Properties

The following invariants are formally proved with Creusot (see `components/dispatcher/verif/`):

| ID | Name | Description |
|----|------|-------------|
| P1 | drive-index-bounded | `drive_index(key, N)` always returns a value < N |
| P2 | eviction-terminates | `evict_for_space` loop exits after at most `max_attempts` iterations |
| P3 | size-validation | `populate` and `prepare_store` reject size == 0 |
| P4 | init-guard | all operations return `NotInitialized` before `initialize()` succeeds |
| P5 | populate-lifecycle | successful populate yields MemoryTier entry with read_ref=1, no write_ref |
| P6 | prepare-commit-lifecycle | prepare creates pending with drive_idx < num_drives; commit produces BlockDevice entry |
| P7 | cancel-removes | `cancel_store` transitions entry to NotExist |
| P8 | drive-index-deterministic | same key always maps to same drive |
| P9 | eviction-progress | each successful eviction strictly decreases memory used |
| P10 | reserve-complete-lifecycle | reserve→copy→complete yields MemoryTier entry with read_ref=1 |

Total: 10 properties, 24 verification conditions discharged by SMT solvers.

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging |
| `dispatch_map` | `IDispatchMap` | Yes | Extent-to-location tracking and reference counting |
| `gpu_services` | `IGpuServices` | Yes | GPU DMA copy operations and stream management |
| `spdk_env` | `ISPDKEnv` | No | SPDK environment; if unconnected, operates in staging-only mode |
| `memory_tier` | `IMemoryTier` | Yes | DRAM pool for caching data between GPU and SSD |
| `remote_lookup` | `IRemoteLookup` | No | Remote node lookup for cache misses |

## Key Types

- `DispatcherConfig { data_pci_addrs, max_cache_entries, eviction_threshold, format_on_init, ssd_eviction_threshold, ssd_eviction_low_watermark, ssd_eviction_batch_size, ssd_eviction_interval_secs, poller_base_cpu, max_eviction_attempts, backfill_delay_ms }`
- `IpcHandle { address: *mut u8, size: u32 }` — opaque GPU memory pointer for DMA transfers
- `DispatcherError` — `NotInitialized`, `KeyNotFound`, `AlreadyExists`, `AllocationFailed`, `IoError`, `Timeout`, `InvalidParameter`
- `ReadWriteStats` — per-direction byte and latency counters aggregated across drives

## Internal Modules

- `background` — parallel memory-tier-to-SSD write-through workers, SSD evictor, memory-tier evictor
- `io_segmenter` — splits large DMA transfers into block-device-aligned segments (128 KiB default)
- `pipeline` — pipelined SSD-to-GPU reads with zero-copy into memory-tier
- `cold_pool` — persistent worker pool with pre-connected NVMe channels + CUDA streams per drive
