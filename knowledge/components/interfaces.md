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
| `IGpuServices` | -- | `initialize`, `shutdown`, `get_devices`, `deserialize_ipc_handle`, `verify_memory`, `pin_memory`, `unpin_memory`, `create_dma_buffer`, `create_stream`, `set_device`, `device_of_ptr`, `destroy_stream`, `stream_query`, `stream_synchronize`; *(spdk)*: `dma_copy_to_host`, `dma_copy_to_device`, `dma_copy_to_device_async`, `memcpy_h2d_async`, `dma_copy_to_host_async`, `memcpy_d2h_async`, `prepare_memory_for_spdk`, `allocate_pinned_dma_buffer`, `register_host_memory`, `unregister_host_memory` |
| `IMemoryTier` | -- | `initialize`, `insert`, `get`, `peek`, `evict_lru`, `evict_lru_for_key`, `oldest_keys`, `remove`, `touch`, `batch_touch`, `contains`, `capacity`, `used`, `pool_info`, `is_dma_capable`, `clear`, `telemetry_snapshot` |
| `IRemoteLookup` | -- | `initialize`, `batch_lookup`, `join_cluster`, `leave_cluster` |
| `IRemoteLookupRdmaInitiator` | -- | `push_async`, `push`, `connect`, `disconnect`, `disconnect_all`, `set_local_peer_id` |
| `IRemoteLookupRdmaResponder` | -- | `open_control_channel`, `local_endpoint`, `local_region` |
| `IRemoteLookupRdmaResponderAdmin` | -- | `set_actor_cpu`, `set_bind_ip`, `initialize`, `signal_stop`, `shutdown` |
| `IZyre` | -- | `ping`, `create_node` |
| `IZyreNode` | -- | `start`, `stop`, `join`, `leave`, `shout`, `whisper`, `recv`, `try_recv`, `uuid`, `name`, `peers`, `peers_by_group`, `own_groups`, `peer_groups`, `peer_address`, `peer_header_value` (plain trait, not `define_interface!`) |
| `IExtendedMetadataStore` | -- | `put`, `get`, `delete`, `iterate_all`, `force_flush` (file exists but not wired in lib.rs) |
| `ISPDKEnv` | `spdk` | `init`, `fini`, `devices`, `device_count`, `is_initialized` |
| `IBlockDevice` | `spdk` | `connect_client`, `sector_size`, `num_sectors`, `max_queue_depth`, `num_io_queues`, `max_transfer_size`, `block_size`, `numa_node`, `nvme_version`, `telemetry`, `read_write_stats` |
| `IBlockDeviceAdmin` | `spdk` | `set_pci_address`, `set_actor_cpu`, `initialize`, `signal_stop`, `shutdown`, `detach_controller` |
| `IExtentManager` | `spdk` | `format`, `initialize`, `reserve_extent`, `get_extents`, `for_each_extent`, `remove_extent`, `checkpoint`, `get_instance_id`, `set_checkpoint_interval`, `used_bytes`, `capacity_bytes`, `set_metadata_base_lba`, `set_data_base_lba`, `data_base_lba` |
| `IDispatchMap` | `spdk` | `initialize`, `lookup`, `convert_to_storage`, `take_read`, `take_write`, `release_read`, `release_write`, `downgrade_reference`, `remove`, `touch`, `entry_size`, `oldest_keys`, `create_memory_tier_entry`, `convert_memory_tier_to_block`, `promote_block_to_memory_tier`, `is_evictable`, `try_evict_to_block`, `recover_extent` |
| `IDispatcher` | `spdk` | `initialize`, `shutdown`, `lookup`, `lookup_async`, `batch_lookup`, `check`, `remove`, `populate`, `reserve_memory`, `copy_gpu_to_memory_async`, `copy_gpu_to_memory_completed`, `release_memory`, `pin`, `unpin`, `touch`, `promote_to_memory_tier`, `clear_memory_tier`, `flush_to_ssd`, `read_write_stats` |
| `IPartitionTable` | `spdk` | `initialize`, `format`, `partition_info`, `num_partitions` |

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

### Block Device
- `DmaBuffer` — DMA-safe hugepage buffer with pluggable allocator/deallocator
- `DmaAllocFn` — `Arc<dyn Fn(usize, usize, Option<i32>) -> Result<DmaBuffer, String> + Send + Sync>`
- `NvmeBlockError`, `TelemetrySnapshot`, `ReadWriteStats`, `OpHandle(u64)`, `NamespaceInfo`
- `ClientChannels { command_tx: Sender<Command>, completion_rx: Receiver<Completion> }`
- `Command` enum: ReadSync, WriteSync, ReadAsync, WriteAsync, WriteZeros, BatchSubmit, AbortOp, NsProbe, NsCreate, NsFormat, NsDelete, ControllerReset
- `Completion` enum: ReadDone, WriteDone, WriteZerosDone, AbortAck, Timeout, NsProbeResult, NsCreated, NsFormatted, NsDeleted, ResetDone, Error

### Dispatch Map / Memory Tier
- `CacheKey = u64`, `LookupResult`, `DispatchMapError`, `MemoryTierError`, `MemoryTierTelemetrySnapshot`

### Extent Manager
- `ExtentKey = u64`, `Extent { key, size, offset }`, `FormatParams`, `WriteHandle`, `ExtentManagerError`

### Eviction Policy
- `PoolId = u32`, `EvictionHandle { pool_id, index }`, `EvictionPolicyError`

### Dispatcher
- `DispatcherConfig`, `IpcHandle { address: *mut u8, size: u32 }`, `DispatcherError`

### GPU Services
- `GpuDeviceInfo`, `GpuIpcHandle`, `GpuDmaBuffer`, `GpuStream`

### Remote Lookup
- `LookupConfig`, `RemoteLookupError`
- `RemoteRegion`, `PushStatus`, `PushCompletion` (`Box<dyn FnOnce(Vec<PushStatus>) + Send>` — `push_async` completion callback; owns whatever keeps the source buffers alive), `RemoteLookupRdmaInitiatorError`
- `Endpoint`, `LocalRegion`, `ControlChannel`, `ResponderCommand`, `ResponderEvent`, `RemoteLookupRdmaResponderError`

### Partition Table
- `PartitionInfo`, `PartitionSpec`, `PartitionConfig`, `PartitionTable`, `PartitionTableError`
- `type_guids`: `CERTUS_METADATA`, `CERTUS_DATA`, `CERTUS_EXTERNAL_META`

### Zyre
- `PeerId`, `ZyreEvent`, `ZyreError`, `NodeConfig`, `GossipConfig`

### SPDK Environment
- `PciAddress`, `PciId`, `VfioDevice`, `SpdkEnvError`

## Receptacles

None (trait definition crate only).
