# interfaces

**Crate**: `interfaces`
**Path**: `components/interfaces/`
**Version**: 0.1.0

## Description

Centralized repository for all component interface trait definitions. Allows components to depend on interface definitions without coupling to implementation crates. SPDK-dependent interfaces are gated behind `features = ["spdk"]`.

## Interfaces Defined

| Interface | Feature | Methods |
|-----------|---------|---------|
| `IGreeter` | -- | `greeting_prefix(&self) -> &str` |
| `ILogger` | -- | `error`, `warn`, `info`, `debug` (all `&self, msg: &str`) |
| `IGpuServices` | -- | `initialize`, `shutdown`, `get_devices`, `deserialize_ipc_handle`, `verify_memory`, `pin_memory`, `unpin_memory`, `create_dma_buffer`; *(spdk)*: `dma_copy_to_host`, `dma_copy_to_device`, `prepare_memory_for_spdk` |
| `IMemoryTier` | `spdk` | `initialize(pool_size)`, `insert(key, size)`, `get(key)`, `evict_lru()`, `remove(key)`, `touch(key)`, `contains(key)`, `capacity()`, `used()`, `pool_info()` |
| `ISPDKEnv` | `spdk` | `init()`, `devices()`, `device_count()`, `is_initialized()` |
| `IBlockDevice` | `spdk` | `connect_client()`, `sector_size(ns_id)`, `num_sectors(ns_id)`, `max_queue_depth()`, `num_io_queues()`, `max_transfer_size()`, `block_size()`, `numa_node()`, `nvme_version()`, `telemetry()` |
| `IBlockDeviceAdmin` | `spdk` | `set_pci_address(addr)`, `set_actor_cpu(cpu)`, `initialize()`, `shutdown()` |
| `IExtentManager` | `spdk` | `format(params)`, `initialize`, `reserve_extent(key, size)`, `lookup_extent(key)`, `get_extents`, `for_each_extent(cb)`, `remove_extent(key)`, `checkpoint`, `get_instance_id` |
| `IDispatchMap` | `spdk` | `set_dma_alloc`, `initialize`, `create_staging`, `create_memory_tier_entry`, `lookup`, `convert_to_storage`, `convert_memory_tier_to_block`, `take_read`, `take_write`, `release_read`, `release_write`, `downgrade_reference`, `remove`, `touch`, `oldest_keys` |
| `IDispatcher` | `spdk` | `initialize(config)`, `shutdown()`, `lookup(key, ipc_handle)`, `check(key)`, `remove(key)`, `populate(key, ipc_handle)` |

## Key Shared Types

### General
- `PciAddress` -- PCI BDF address (`domain`, `bus`, `dev`, `func`)
- `PciId` -- vendor/device/class IDs
- `VfioDevice` -- SPDK-discovered NVMe device with address, id, numa_node

### Block Device
- `DmaBuffer` -- DMA-safe hugepage buffer with pluggable allocator/deallocator
- `DmaAllocFn` -- `Arc<dyn Fn(usize, usize, Option<i32>) -> Result<DmaBuffer, String> + Send + Sync>`
- `NvmeBlockError` -- error enum: `FeatureNotEnabled`, `NotInitialized`, `Timeout`, `Aborted`, `InvalidNamespace`, `NotSupported`, `BlockDevice`, `SpdkEnv`, `LbaOutOfRange`, `ClientDisconnected`
- `TelemetrySnapshot` -- `{total_ops, min/max/mean_latency_ns, mean_throughput_mbps, elapsed_secs}`
- `OpHandle(u64)` -- async operation handle
- `NamespaceInfo` -- `{ns_id, num_sectors, sector_size}`
- `ClientChannels` -- `{command_tx: Sender<Command>, completion_rx: Receiver<Completion>}`

### Messaging Protocol
- `Command` enum: `ReadSync`, `WriteSync`, `ReadAsync`, `WriteAsync`, `WriteZeros`, `BatchSubmit`, `AbortOp`, `NsProbe`, `NsCreate`, `NsFormat`, `NsDelete`, `ControllerReset`
- `Completion` enum: `ReadDone`, `WriteDone`, `WriteZerosDone`, `AbortAck`, `Timeout`, `NsProbeResult`, `NsCreated`, `NsFormatted`, `NsDeleted`, `ResetDone`, `Error`

### Extent Manager
- `ExtentKey = u64`
- `Extent { key, size, offset }`
- `FormatParams`, `WriteHandle`
- `ExtentManagerError`

### Dispatch Map
- `CacheKey = u64`
- `LookupResult` -- `NotExist`, `MismatchSize`, `Staging { buffer }`, `BlockDevice { offset }`, `MemoryTier { pointer, size }`
- `DispatchMapError`

### Memory Tier
- `MemoryTierError` -- `PoolFull`, `KeyNotFound`, `AlreadyExists`, `AllocationFailed`, `InvalidSize`, `NotEvictable`, `NotInitialized`

### Dispatcher
- `DispatcherConfig { metadata_pci_addr, data_pci_addrs }`
- `IpcHandle { address: *mut u8, size: u32 }` -- opaque GPU memory handle for DMA
- `DispatcherError`

### GPU Services
- `GpuDeviceInfo { device_index, name, memory_bytes, compute_major, compute_minor, pci_bus_id }`
- `GpuIpcHandle` -- opened CUDA IPC handle with verification/pinning state
- `GpuDmaBuffer` -- owns GPU device pointer, calls `cudaIpcCloseMemHandle` on drop

## Receptacles

None (trait definition crate only).
