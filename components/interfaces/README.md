# interfaces

Centralized interface trait definitions for the Certus component system. Components depend on this crate for shared trait contracts rather than depending on each other directly, keeping coupling low and enabling independent development.

## Interfaces

### Always Available

| Interface | Methods | Purpose |
|-----------|---------|---------|
| `IGreeter` | `greeting_prefix(&self) -> &str` | Example interface for demos |
| `ILogger` | `error()`, `warn()`, `info()`, `debug()` | Structured logging |

### Always Available Types

| Type | Purpose |
|------|---------|
| `Extent` | Storage extent record (key, size, offset) |
| `ExtentKey` | Alias for `u64` identifying an extent |
| `ExtentManagerError` | Error type for extent manager operations |

### SPDK Feature (`--features spdk`)

| Interface | Methods | Purpose |
|-----------|---------|---------|
| `ISPDKEnv` | `init()`, `devices()`, `device_count()`, `is_initialized()` | SPDK environment lifecycle |
| `IBlockDevice` | `connect_client()`, `sector_size()`, `num_sectors()`, `max_queue_depth()`, `num_io_queues()`, `max_transfer_size()`, `block_size()`, `numa_node()`, `nvme_version()`, `telemetry()` | NVMe block device client access |
| `IBlockDeviceAdmin` | `set_pci_address()`, `initialize()` | NVMe controller configuration |
| `IExtentManager` | `set_dma_alloc()`, `initialize()`, `create_extent()`, `remove_extent()`, `lookup_extent()`, `get_extents()`, `for_each_extent()` | Extent allocation and lifecycle |
| `IExtentManagerV2` | `set_dma_alloc()`, `format()`, `initialize()`, `reserve_extent()`, `lookup_extent()`, `remove_extent()`, `get_extents()`, `for_each_extent()`, `checkpoint()` | Two-phase extent allocation with crash-consistent checkpointing |

### SPDK Types

When the `spdk` feature is enabled, the crate also exports supporting types:

- `Command`, `Completion` — NVMe IO message types
- `DmaBuffer`, `DmaAllocFn` — DMA-safe memory with pluggable allocators
- `PciAddress`, `PciId`, `VfioDevice` — device identification
- `TelemetrySnapshot`, `OpHandle`, `NamespaceInfo`, `ClientChannels` — block device support types
- `SpdkEnvError`, `BlockDeviceError`, `NvmeBlockError` — error types
- `FormatParams` — configuration for extent manager v2 formatting (slab size, chunk size, region count, etc.)
- `WriteHandle` — RAII handle from `reserve_extent`; call `.publish()` to commit or `.abort()` (or drop) to release

## Build

```bash
# Default build (no SPDK types)
cargo build -p interfaces

# With SPDK interfaces and types
cargo build -p interfaces --features spdk
```

## Test

```bash
cargo test -p interfaces
```

## Source Layout

```
src/
  lib.rs                  Module declarations and re-exports
  igreeter.rs             IGreeter interface
  ilogger.rs              ILogger interface
  iextent_manager.rs      IExtentManager interface + Extent, ExtentKey, ExtentManagerError
  iextent_manager_v2.rs   IExtentManagerV2 interface + FormatParams, WriteHandle (feature = "spdk")
  ispdk_env.rs            ISPDKEnv interface (feature = "spdk")
  iblock_device.rs        IBlockDevice, IBlockDeviceAdmin (feature = "spdk")
  spdk_types.rs           DmaBuffer, PciAddress, error types (feature = "spdk")
```
