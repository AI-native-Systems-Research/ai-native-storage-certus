# Interfaces

Centralized interface trait definitions for the Certus component system. Components depend on this crate for shared trait contracts rather than depending on each other directly, keeping coupling low and enabling independent development.

## Summary

Defines all shared interface traits (`IBlockDevice`, `IDispatcher`, `IGpuServices`, `IDispatchMap`, `IMemoryTier`, etc.) and their associated types. SPDK-dependent interfaces are gated behind the `spdk` feature; GPU types behind the `gpu` feature.

## Structure

```
src/
  lib.rs                Module declarations and re-exports
  igreeter.rs           IGreeter interface (example)
  ilogger.rs            ILogger interface (error, warn, info, debug)
  idispatcher.rs        IDispatcher interface + DispatcherConfig, DispatcherError
  idispatch_map.rs      IDispatchMap interface + CacheKey, LookupResult, DispatchMapError
  igpu_services.rs      IGpuServices interface + GpuDeviceInfo, GpuIpcHandle, GpuDmaBuffer, GpuStream
  imemory_tier.rs       IMemoryTier interface + MemoryTierError
  iextent_manager.rs    IExtentManager interface + Extent, ExtentKey, FormatParams, WriteHandle
  ispdk_env.rs          ISPDKEnv interface (feature = "spdk")
  iblock_device.rs      IBlockDevice, IBlockDeviceAdmin (feature = "spdk")
  spdk_types.rs         DmaBuffer, PciAddress, PciId, VfioDevice, error types (feature = "spdk")
```

### Always Available

| Interface / Type | Purpose |
|------------------|---------|
| `IGreeter` | Example interface for demos |
| `ILogger` | Structured logging (error, warn, info, debug) |
| `IGpuServices` | GPU initialization, IPC handles, DMA copies, streams |
| `GpuDeviceInfo`, `GpuIpcHandle`, `GpuDmaBuffer`, `GpuStream` | GPU-related types |
| `DispatcherConfig`, `DispatcherError` | Dispatcher configuration and errors |
| `CacheKey`, `DispatchMapError` | Dispatch map key and error types |
| `MemoryTierError` | Memory-tier error type |
| `Extent`, `ExtentKey`, `ExtentManagerError`, `FormatParams`, `WriteHandle` | Extent manager types |

### Feature-Gated (`spdk`)

| Interface | Purpose |
|-----------|---------|
| `ISPDKEnv` | SPDK environment lifecycle |
| `IBlockDevice` | NVMe block device client access |
| `IBlockDeviceAdmin` | NVMe controller configuration |
| `IExtentManager` | Extent allocation and lifecycle |
| `IDispatchMap` | Cache key-to-location mapping with reference locking |
| `IDispatcher` | Cache orchestration (populate, lookup, check, remove) |
| `IMemoryTier` | DRAM pool allocation with LRU eviction |

### Feature-Gated (`gpu`)

Enables GPU-specific associated types used by `IGpuServices` (e.g., CUDA IPC handle payload structures).

## Build and Test

```bash
# Default build (no SPDK types)
cargo build -p interfaces

# With SPDK interfaces and types
cargo build -p interfaces --features spdk

# With GPU types
cargo build -p interfaces --features gpu

# Tests
cargo test -p interfaces

# Lint and docs
cargo clippy -p interfaces -- -D warnings
cargo doc -p interfaces --no-deps
```
