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
| `IEvictionPolicy` | -- | `create_pool`, `track`, `touch`, `batch_touch`, `remove`, `pop_oldest`, `peek_oldest`, `len`, `clear_pool` |
| `IGpuServices` | -- | `initialize`, `shutdown`, `get_devices`, `deserialize_ipc_handle`, `verify_memory`, `pin_memory`, `unpin_memory`, `create_dma_buffer`, `create_stream`, `destroy_stream`, `stream_query`, `stream_synchronize`; *(spdk)*: `dma_copy_to_host`, `dma_copy_to_device`, `dma_copy_to_device_async`, `memcpy_h2d_async`, `dma_copy_to_host_async`, `memcpy_d2h_async`, `prepare_memory_for_spdk`, `allocate_pinned_dma_buffer`, `register_host_memory`, `unregister_host_memory` |
| `IRemoteLookup` | -- | `batch_lookup`, `join_cluster`, `leave_cluster` |
| `IRemoteRequestHandler` | -- | `handle_lookup`, `handle_check`, `handle_batch_lookup`, `release_lookup` |
| `IMemoryTier` | `spdk` | `initialize`, `insert`, `get`, `peek`, `evict_lru`, `evict_lru_for_key`, `oldest_keys`, `remove`, `touch`, `batch_touch`, `contains`, `capacity`, `used`, `pool_info`, `is_dma_capable`, `clear` |
| `ISPDKEnv` | `spdk` | `init`, `fini`, `devices`, `device_count`, `is_initialized` |
| `IBlockDevice` | `spdk` | `connect_client`, `sector_size`, `num_sectors`, `max_queue_depth`, `num_io_queues`, `max_transfer_size`, `block_size`, `numa_node`, `nvme_version`, `telemetry` |
| `IBlockDeviceAdmin` | `spdk` | `set_pci_address`, `set_actor_cpu`, `initialize`, `signal_stop`, `shutdown`, `detach_controller` |
| `IExtentManager` | `spdk` | `format`, `initialize`, `reserve_extent`, `get_extents`, `for_each_extent`, `remove_extent`, `checkpoint`, `get_instance_id`, `set_checkpoint_interval`, `used_bytes`, `capacity_bytes` |
| `IDispatchMap` | `spdk` | `set_dma_alloc`, `initialize`, `create_staging`, `lookup`, `convert_to_storage`, `take_read`, `take_write`, `release_read`, `release_write`, `downgrade_reference`, `remove`, `touch`, `entry_size`, `oldest_keys`, `create_memory_tier_entry`, `convert_memory_tier_to_block`, `is_evictable`, `recover_extent` |
| `IDispatcher` | `spdk` | `initialize`, `shutdown`, `lookup`, `lookup_async`, `batch_lookup`, `check`, `remove`, `populate`, `reserve_memory`, `copy_gpu_to_memory_async`, `copy_gpu_to_memory_completed`, `release_memory`, `prepare_store`, `commit_store`, `cancel_store`, `touch`, `promote_to_memory_tier`, `clear_memory_tier`, `flush_to_ssd` |

## Verified Properties Summary

| Interface | Properties | Verification Conditions |
|-----------|-----------|------------------------|
| `IExtentManager` | 10 (P1–P10) | 22 discharged by SMT |
| `IGpuServices` | 10 (P1–P10) | 19 discharged by SMT |
| `IMemoryTier` | 10 (P1–P10) | 21 discharged by SMT |
| `IDispatchMap` | 10 (P1–P10) | 24 discharged by SMT |
| `IDispatcher` | 10 (P1–P10) | 24 discharged by SMT |
| **Total** | **50** | **110** |

## Key Shared Types

### General
- `PciAddress` — PCI BDF address (`domain`, `bus`, `dev`, `func`)
- `PciId` — vendor/device/class IDs
- `VfioDevice` — SPDK-discovered NVMe device with address, id, numa_node

### Block Device
- `DmaBuffer` — DMA-safe hugepage buffer with pluggable allocator/deallocator
- `DmaAllocFn` — `Arc<dyn Fn(usize, usize, Option<i32>) -> Result<DmaBuffer, String> + Send + Sync>`
- `NvmeBlockError` — `FeatureNotEnabled`, `NotInitialized`, `Timeout`, `Aborted`, `InvalidNamespace`, `NotSupported`, `BlockDevice`, `SpdkEnv`, `LbaOutOfRange`, `ClientDisconnected`
- `TelemetrySnapshot` — `{total_ops, min/max/mean_latency_ns, mean_throughput_mbps, elapsed_secs}`
- `OpHandle(u64)` — async operation handle
- `NamespaceInfo` — `{ns_id, num_sectors, sector_size}`
- `ClientChannels` — `{command_tx: Sender<Command>, completion_rx: Receiver<Completion>}`

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
- `LookupResult` — `NotExist`, `MismatchSize`, `Staging { buffer }`, `BlockDevice { offset }`, `MemoryTier { pointer, size }`
- `DispatchMapError`

### Memory Tier
- `MemoryTierError` — `PoolFull`, `KeyNotFound`, `AlreadyExists`, `AllocationFailed`, `InvalidSize`, `NotEvictable`, `NotInitialized`

### Eviction Policy
- `PoolId = u32`
- `EvictionHandle { pool_id, index }`
- `EvictionPolicyError` — `InvalidPool`, `InvalidHandle`

### Dispatcher
- `DispatcherConfig { data_pci_addrs, max_cache_entries, ... }`
- `IpcHandle { address: *mut u8, size: u32 }` — opaque GPU memory handle for DMA
- `DispatcherError`

### GPU Services
- `GpuDeviceInfo { device_index, name, memory_bytes, compute_major, compute_minor, pci_bus_id }`
- `GpuIpcHandle` — opened CUDA IPC handle with verification/pinning state
- `GpuDmaBuffer` — owns GPU device pointer, calls `cudaIpcCloseMemHandle` on drop
- `GpuStream` — opaque CUDA stream handle

### Remote Request Handler
- `LookupRef { ptr: *const u8, size: u32, key: CacheKey }` — zero-copy reference
- `RemoteRequestHandlerError` — `InvalidRequest`, `KeyNotFound`, `DispatchError`, `NotInitialized`

### Remote Lookup
- `RemoteLookupError` — `NotFound`, `TransportError`

## Receptacles

None (trait definition crate only).
