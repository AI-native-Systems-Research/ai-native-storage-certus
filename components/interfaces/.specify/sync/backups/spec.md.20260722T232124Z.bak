# Feature Specification: Shared Interface Trait Definitions

**Feature Branch**: `001-interfaces`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice
> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `interfaces` crate provides centralized trait definitions for all Certus component interfaces. It allows components to depend on interface definitions without pulling in implementation crates, enforcing low coupling and enabling independent development. Interfaces are defined using the `define_interface!` procedural macro from `component-macros`, and all components implement `IUnknown` for runtime interface discovery.

The crate has two Cargo features:
- **`spdk`** (optional): Gates SPDK-dependent interfaces and types (`IBlockDevice`, `IBlockDeviceAdmin`, `ISPDKEnv`, `IDispatcher`, `IDispatchMap`, `IMemoryTier`, `IExtentManager`, `IPartitionTable`) and supporting types (`DmaBuffer`, `PciAddress`, etc.).
- **`gpu`** (optional): Reserved for GPU-specific conditional compilation.

## User Scenarios & Testing

### User Story 1 - Component Developer Adding a New Component (Priority: P1)

**As a** component developer, **I want to** import interface traits from a single crate **so that** I can implement a component without depending on other implementation crates.

**Acceptance Criteria:**
- A new component can depend solely on `interfaces` and `component-core`/`component-macros` to implement any interface.
- Interface traits are defined with `define_interface!` ensuring `IUnknown` integration.
- Types referenced in interface methods (errors, handles, configs) are exported from `interfaces`.

### User Story 2 - Dispatcher Orchestrating Multi-Component I/O (Priority: P1)

**As the** dispatcher component, **I want to** use typed interfaces for block devices, extent managers, memory tiers, and GPU services **so that** I can orchestrate data flow without compile-time coupling to specific implementations.

**Acceptance Criteria:**
- `IDispatcher` provides `initialize`, `shutdown`, `lookup`, `lookup_async`, `batch_lookup`, `check`, `remove`, `populate`, `reserve_memory`, `copy_gpu_to_memory_async`, `copy_gpu_to_memory_completed`, `release_memory`, `touch`, `promote_to_memory_tier`, `clear_memory_tier`, and `flush_to_ssd` methods.
- `IDispatchMap` provides reference-counted cache entry management with read/write locking semantics.
- `IMemoryTier` provides a DRAM pool with LRU eviction, sharded allocation, and NUMA awareness.

### User Story 3 - NVMe Block Device Actor (Priority: P1)

**As the** block device component, **I want** a channel-based interface definition **so that** clients can submit commands and receive completions asynchronously via typed channels.

**Acceptance Criteria:**
- `IBlockDevice` provides `connect_client` returning typed channel endpoints.
- `Command` and `Completion` enums cover all NVMe operations (read/write sync and async, write zeros, batch, abort, namespace management, controller reset).
- `IBlockDeviceAdmin` provides lifecycle management (set PCI address, set CPU, initialize, signal_stop, shutdown, detach_controller).

### User Story 4 - GPU-Accelerated Data Path (Priority: P2)

**As the** GPU services component, **I want** an interface for CUDA IPC handle management and DMA operations **so that** the dispatcher can perform zero-copy GPU-to-SSD and SSD-to-GPU transfers.

**Acceptance Criteria:**
- `IGpuServices` provides initialization, device discovery, IPC handle deserialization, memory verification, pinning, DMA buffer creation, sync/async memory copies in both directions, stream lifecycle, and host memory registration.
- State machine for IPC handles: fresh -> verified -> pinned -> DMA-capable.

### User Story 5 - Remote Cache Lookups (Priority: P2)

**As the** remote lookup component, **I want** interfaces for both outbound lookups and inbound request handling **so that** cache misses can be served from other Certus nodes in a cluster.

**Acceptance Criteria:**
- `IRemoteLookup` provides `batch_lookup`, `join_cluster`, and `leave_cluster`.
- `IRemoteRequestHandler` provides `handle_lookup`, `handle_check`, `handle_batch_lookup`, and `release_lookup` with zero-copy `LookupRef` semantics.

### User Story 6 - Extent Management (Priority: P1)

**As the** extent manager component, **I want** a trait defining extent lifecycle operations **so that** the dispatcher can allocate, write, persist, and remove extents on NVMe devices.

**Acceptance Criteria:**
- `IExtentManager` provides `format`, `initialize`, `reserve_extent`, `get_extents`, `for_each_extent`, `remove_extent`, `checkpoint`, `get_instance_id`, `set_checkpoint_interval`, `used_bytes`, `capacity_bytes`, `set_metadata_base_lba`, `set_data_base_lba`, and `data_base_lba`.
- `WriteHandle` implements publish/abort semantics with auto-abort on drop.

## Requirements

### Functional Requirements

#### FR-001: IGreeter Interface
- **Method**: `greeting_prefix(&self) -> &str`
- Returns a string prefix for greeting messages.

#### FR-002: ILogger Interface
- **Method**: `error(&self, msg: &str)` - Log an error message.
- **Method**: `warn(&self, msg: &str)` - Log a warning message.
- **Method**: `info(&self, msg: &str)` - Log an informational message.
- **Method**: `debug(&self, msg: &str)` - Log a debug message.

#### FR-003: ISPDKEnv Interface (feature: spdk)
- **Method**: `init(&self) -> Result<(), SpdkEnvError>` - Initialize SPDK/DPDK environment and discover VFIO devices.
- **Method**: `fini(&self)` - Tear down SPDK/DPDK environment.
- **Method**: `devices(&self) -> Vec<VfioDevice>` - Return all probed VFIO devices.
- **Method**: `device_count(&self) -> usize` - Return number of discovered devices.
- **Method**: `is_initialized(&self) -> bool` - Check initialization state.

#### FR-004: IBlockDevice Interface (feature: spdk)
- **Method**: `connect_client(&self) -> Result<ClientChannels, NvmeBlockError>` - Create channel-based client connection.
- **Method**: `sector_size(&self, ns_id: u32) -> Result<u32, NvmeBlockError>` - Return sector size for a namespace.
- **Method**: `num_sectors(&self, ns_id: u32) -> Result<u64, NvmeBlockError>` - Return total sectors for a namespace.
- **Method**: `max_queue_depth(&self) -> u32` - Return maximum queue depth.
- **Method**: `num_io_queues(&self) -> u32` - Return number of I/O queues.
- **Method**: `max_transfer_size(&self) -> u32` - Return maximum data transfer size in bytes.
- **Method**: `block_size(&self) -> u32` - Return block/sector size for default namespace.
- **Method**: `numa_node(&self) -> i32` - Return NUMA node ID of the controller.
- **Method**: `nvme_version(&self) -> String` - Return NVMe specification version.
- **Method**: `telemetry(&self) -> Result<TelemetrySnapshot, NvmeBlockError>` - Return telemetry statistics.

#### FR-005: IBlockDeviceAdmin Interface (feature: spdk)
- **Method**: `set_pci_address(&self, addr: PciAddress)` - Set PCI address for controller attachment.
- **Method**: `set_actor_cpu(&self, cpu: usize)` - Pin actor thread to a CPU core.
- **Method**: `initialize(&self) -> Result<(), NvmeBlockError>` - Start the actor thread.
- **Method**: `signal_stop(&self)` - Signal the actor to stop without joining.
- **Method**: `shutdown(&self) -> Result<(), NvmeBlockError>` - Stop actor and join thread.
- **Method**: `detach_controller(&self)` - Detach NVMe controller from SPDK.

#### FR-006: IEvictionPolicy Interface
- **Method**: `create_pool(&self) -> PoolId` - Create a new eviction tracking pool.
- **Method**: `track(&self, pool: PoolId, key: CacheKey) -> Result<EvictionHandle, EvictionPolicyError>` - Register a key as most-recently-used.
- **Method**: `touch(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError>` - Mark entry as most-recently-used.
- **Method**: `batch_touch(&self, handles: &[EvictionHandle]) -> Result<(), EvictionPolicyError>` - Batch MRU update.
- **Method**: `remove(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError>` - Stop tracking entry.
- **Method**: `pop_oldest(&self, pool: PoolId) -> Option<CacheKey>` - Remove and return LRU key.
- **Method**: `peek_oldest(&self, pool: PoolId, n: usize) -> Vec<CacheKey>` - Peek at N oldest keys.
- **Method**: `len(&self, pool: PoolId) -> usize` - Return tracked entry count.
- **Method**: `clear_pool(&self, pool: PoolId)` - Remove all entries from pool.

#### FR-007: IDispatchMap Interface (feature: spdk)
- **Method**: `initialize(&self) -> Result<(), DispatchMapError>` - Recover committed extents.
- **Method**: `lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError>` - Look up key, blocking if writer active.
- **Method**: `convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError>` - Transition memory-tier entry to block-device location.
- **Method**: `take_read(&self, key: CacheKey) -> Result<(), DispatchMapError>` - Acquire read reference.
- **Method**: `take_write(&self, key: CacheKey) -> Result<(), DispatchMapError>` - Acquire write reference (exclusive).
- **Method**: `release_read(&self, key: CacheKey) -> Result<(), DispatchMapError>` - Release read reference.
- **Method**: `release_write(&self, key: CacheKey) -> Result<(), DispatchMapError>` - Release write reference.
- **Method**: `downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError>` - Downgrade write to read reference.
- **Method**: `remove(&self, key: CacheKey) -> Result<(), DispatchMapError>` - Remove entry (requires zero refs).
- **Method**: `touch(&self, key: CacheKey) -> Result<(), DispatchMapError>` - Update timestamp.
- **Method**: `entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError>` - Return stored size.
- **Method**: `oldest_keys(&self, n: usize) -> Vec<CacheKey>` - Return N oldest keys.
- **Method**: `create_memory_tier_entry(&self, key: CacheKey, pointer: *mut u8, size: u32) -> Result<(), DispatchMapError>` - Create entry with write ref.
- **Method**: `convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError>` - Convert memory-tier to block-device state.
- **Method**: `is_evictable(&self, key: CacheKey) -> bool` - Check if entry can be evicted.
- **Method**: `recover_extent(&self, key: CacheKey, offset: u64, size_blocks: u32) -> Result<(), DispatchMapError>` - Insert recovered extent.

#### FR-008: IDispatcher Interface (feature: spdk)
- **Method**: `initialize(&self, config: DispatcherConfig) -> Result<(), DispatcherError>` - Initialize with PCI addresses and config.
- **Method**: `shutdown(&self) -> Result<(), DispatcherError>` - Shut down, completing in-flight writes.
- **Method**: `lookup(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>` - Look up and DMA-copy to GPU memory.
- **Method**: `lookup_async(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<GpuStream, DispatcherError>` - Async lookup returning CUDA stream.
- **Method**: `batch_lookup(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>` - Concurrent batch lookup.
- **Method**: `check(&self, key: CacheKey) -> Result<bool, DispatcherError>` - Check entry existence.
- **Method**: `remove(&self, key: CacheKey) -> Result<(), DispatcherError>` - Remove entry and free resources.
- **Method**: `populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>` - Populate cache from GPU memory.
- **Method**: `reserve_memory(&self, key: CacheKey, size: u32) -> Result<*mut u8, DispatcherError>` - Reserve memory-tier slot.
- **Method**: `copy_gpu_to_memory_async(&self, key: CacheKey, ipc_handle: IpcHandle, stream: GpuStream) -> Result<(), DispatcherError>` - DMA copy into reserved slot.
- **Method**: `copy_gpu_to_memory_completed(&self, key: CacheKey, size: u32) -> Result<(), DispatcherError>` - Finalize populated slot.
- **Method**: `release_memory(&self, key: CacheKey) -> Result<(), DispatcherError>` - Release reserved slot (cancellation path).
- **Method**: `touch(&self, key: CacheKey) -> Result<(), DispatcherError>` - Refresh eviction timestamp.
- **Method**: `promote_to_memory_tier(&self, keys: &[CacheKey])` - Promote SSD-resident entries to DRAM.
- **Method**: `clear_memory_tier(&self) -> Result<usize, DispatcherError>` - Evict all memory-tier entries.
- **Method**: `flush_to_ssd(&self) -> Result<usize, DispatcherError>` - Flush pending writes to SSD.

#### FR-009: IMemoryTier Interface (feature: spdk)
- **Method**: `initialize(&self, pool_size: usize, numa_node: Option<i32>) -> Result<(), MemoryTierError>` - Initialize pool with NUMA binding.
- **Method**: `insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError>` - Allocate slot.
- **Method**: `get(&self, key: CacheKey) -> Option<(*mut u8, u32)>` - Get pointer with LRU refresh.
- **Method**: `peek(&self, key: CacheKey) -> Option<(*mut u8, u32)>` - Get pointer without LRU update.
- **Method**: `evict_lru(&self) -> Option<CacheKey>` - Evict oldest entry.
- **Method**: `evict_lru_for_key(&self, key: CacheKey) -> Option<CacheKey>` - Evict from target shard.
- **Method**: `oldest_keys(&self, n: usize) -> Vec<CacheKey>` - Peek N oldest.
- **Method**: `remove(&self, key: CacheKey) -> Result<(), MemoryTierError>` - Remove specific entry.
- **Method**: `touch(&self, key: CacheKey)` - Update LRU position.
- **Method**: `batch_touch(&self, keys: &[CacheKey])` - Batch LRU update.
- **Method**: `contains(&self, key: CacheKey) -> bool` - Check existence.
- **Method**: `capacity(&self) -> usize` - Return total pool capacity.
- **Method**: `used(&self) -> usize` - Return bytes allocated.
- **Method**: `pool_info(&self) -> Option<(*mut u8, usize)>` - Return pool base pointer and size.
- **Method**: `is_dma_capable(&self) -> bool` - Check if pool supports direct NVMe DMA.
- **Method**: `clear(&self) -> Result<usize, MemoryTierError>` - Remove all entries.

#### FR-010: IExtentManager Interface (feature: spdk)
- **Method**: `format(&self, params: FormatParams) -> Result<(), ExtentManagerError>` - Format with validated params.
- **Method**: `initialize(&self) -> Result<(), ExtentManagerError>` - Recover from persisted metadata.
- **Method**: `reserve_extent(&self, key: ExtentKey, size: u32) -> Result<WriteHandle, ExtentManagerError>` - Reserve space, return WriteHandle.
- **Method**: `get_extents(&self) -> Vec<Extent>` - Return all committed extents.
- **Method**: `for_each_extent(&self, cb: &mut dyn FnMut(&Extent))` - Iterate committed extents.
- **Method**: `remove_extent(&self, offset: u64) -> Result<(), ExtentManagerError>` - Remove extent at offset.
- **Method**: `checkpoint(&self) -> Result<(), ExtentManagerError>` - Persist state to metadata device.
- **Method**: `get_instance_id(&self) -> Result<u64, ExtentManagerError>` - Return superblock instance ID.
- **Method**: `set_checkpoint_interval(&self, interval: Option<std::time::Duration>)` - Configure auto-checkpoint.
- **Method**: `used_bytes(&self) -> u64` - Return bytes allocated.
- **Method**: `capacity_bytes(&self) -> u64` - Return total usable capacity.
- **Method**: `set_metadata_base_lba(&self, base_lba: u64)` - Set metadata I/O base LBA.
- **Method**: `set_data_base_lba(&self, base_lba: u64)` - Set data partition base LBA.
- **Method**: `data_base_lba(&self) -> u64` - Get data base LBA.

#### FR-011: IGpuServices Interface
- **Method**: `initialize(&self) -> Result<(), String>` - Init CUDA and discover GPUs.
- **Method**: `shutdown(&self) -> Result<(), String>` - Shut down CUDA context.
- **Method**: `get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String>` - List qualifying GPUs.
- **Method**: `deserialize_ipc_handle(&self, base64_payload: &str) -> Result<GpuIpcHandle, String>` - Open CUDA IPC handle.
- **Method**: `verify_memory(&self, handle: &GpuIpcHandle) -> Result<(), String>` - Verify device memory suitability.
- **Method**: `pin_memory(&self, handle: &GpuIpcHandle) -> Result<(), String>` - Pin GPU memory for DMA.
- **Method**: `unpin_memory(&self, handle: &GpuIpcHandle) -> Result<(), String>` - Unpin GPU memory.
- **Method**: `create_dma_buffer(&self, handle: GpuIpcHandle) -> Result<GpuDmaBuffer, String>` - Create DMA buffer from IPC handle.
- **Method**: `dma_copy_to_host(&self, src, dst, size) -> Result<(), String>` (feature: spdk) - Sync GPU-to-host copy.
- **Method**: `dma_copy_to_device(&self, src, dst, size) -> Result<(), String>` (feature: spdk) - Sync host-to-GPU copy.
- **Method**: `prepare_memory_for_spdk(&self, base64_payload, device_index) -> Result<DmaBuffer, String>` (feature: spdk) - One-call P2P preparation.
- **Method**: `create_stream(&self) -> Result<GpuStream, String>` - Create CUDA stream.
- **Method**: `set_device(&self, device: i32) -> Result<(), String>` - Select the calling thread's current CUDA device.
- **Method**: `device_of_ptr(&self, ptr: *const c_void) -> Result<i32, String>` - Return the CUDA device ordinal owning a device pointer (-1 if unknown).
- **Method**: `destroy_stream(&self, stream: GpuStream) -> Result<(), String>` - Destroy CUDA stream.
- **Method**: `stream_query(&self, stream: GpuStream) -> Result<bool, String>` - Non-blocking completion check.
- **Method**: `stream_synchronize(&self, stream: GpuStream) -> Result<(), String>` - Block until stream completes.
- **Method**: `dma_copy_to_device_async(&self, src, dst, size, stream) -> Result<(), String>` (feature: spdk) - Async host-to-GPU copy.
- **Method**: `memcpy_h2d_async(&self, src, dst, size, stream) -> Result<(), String>` (feature: spdk) - Async raw-pointer H2D copy.
- **Method**: `dma_copy_to_host_async(&self, src, dst, size, stream) -> Result<(), String>` (feature: spdk) - Async GPU-to-host copy.
- **Method**: `memcpy_d2h_async(&self, src, dst, size, stream) -> Result<(), String>` (feature: spdk) - Async raw-pointer D2H copy.
- **Method**: `allocate_pinned_dma_buffer(&self, size) -> Result<DmaBuffer, String>` (feature: spdk) - Allocate CUDA-pinned + SPDK-registered buffer.
- **Method**: `register_host_memory(&self, ptr, size) -> Result<(), String>` (feature: spdk) - Register existing memory for DMA.
- **Method**: `unregister_host_memory(&self, ptr, size) -> Result<(), String>` (feature: spdk) - Unregister host memory.

#### FR-012: IRemoteLookup Interface
- **Method**: `batch_lookup(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), RemoteLookupError>>` - Batch lookup from remote nodes.
- **Method**: `join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError>` - Join a cluster.
- **Method**: `leave_cluster(&self) -> Result<(), RemoteLookupError>` - Leave the cluster.

#### FR-013: IRemoteRequestHandler Interface
- **Method**: `handle_lookup(&self, key: CacheKey) -> Result<LookupRef, RemoteRequestHandlerError>` - Zero-copy lookup returning pinned reference.
- **Method**: `handle_check(&self, key: CacheKey) -> Result<bool, RemoteRequestHandlerError>` - Check key existence.
- **Method**: `handle_batch_lookup(&self, keys: &[CacheKey]) -> Vec<Result<LookupRef, RemoteRequestHandlerError>>` - Batch zero-copy lookup.
- **Method**: `release_lookup(&self, key: CacheKey)` - Release read reference after data consumption.

#### FR-014: IExtendedMetadataStore Interface
- **Method**: `put(&self, key: &str, value: &[u8]) -> Result<(), ExtendedMetadataStoreError>` - Store metadata entry.
- **Method**: `get(&self, key: &str) -> Result<Vec<u8>, ExtendedMetadataStoreError>` - Retrieve metadata entry.
- **Method**: `delete(&self, key: &str) -> Result<(), ExtendedMetadataStoreError>` - Delete metadata entry.
- **Method**: `iterate_all(&self) -> Result<Vec<(String, Vec<u8>)>, ExtendedMetadataStoreError>` - Iterate all entries.
- **Method**: `force_flush(&self) -> Result<(), ExtendedMetadataStoreError>` - Flush to persistent storage.

#### FR-015: IPartitionTable Interface (feature: spdk)
- **Method**: `initialize(&self) -> Result<PartitionTable, PartitionTableError>` - Read and validate GPT.
- **Method**: `format(&self, config: PartitionConfig) -> Result<PartitionTable, PartitionTableError>` - Write new GPT layout.
- **Method**: `partition_info(&self, index: u32) -> Result<PartitionInfo, PartitionTableError>` - Get partition info by index.
- **Method**: `num_partitions(&self) -> Result<u32, PartitionTableError>` - Return partition count.

#### FR-016: Supporting Types - SPDK (feature: spdk)
- `DmaBuffer`: DMA-safe buffer with pluggable allocator, NUMA awareness, metadata map, Deref/DerefMut to `[u8]`.
- `PciAddress`: Bus-Device-Function address with display formatting.
- `PciId`: Vendor/device/class identification.
- `VfioDevice`: Discovered VFIO device snapshot.
- `SpdkEnvError`: Environment initialization errors (7 variants).
- `BlockDeviceError`: Block device operation errors (10 variants).
- `DmaAllocFn`: Pluggable allocator type alias.
- `set_spdk_env_active` / `is_spdk_env_active`: Global SPDK lifecycle flag.

#### FR-017: Supporting Types - Block Device (feature: spdk)
- `NvmeBlockError`: NVMe operation errors (10 variants).
- `TelemetrySnapshot`: IO statistics (ops, latency, throughput).
- `OpHandle`: Unique async operation identifier.
- `NamespaceInfo`: NVMe namespace metadata.
- `Command`: 11-variant enum for all NVMe operations.
- `Completion`: 10-variant enum for operation results; derives `Clone` (in addition to `Debug`) so the block-device actor can `try_send` a clone of a completion on a full ring without consuming the original, enabling non-blocking completion delivery.
- `ClientChannels`: Channel pair (command_tx, completion_rx).

#### FR-018: Supporting Types - Dispatcher
- `DispatcherConfig`: 16-field configuration (PCI addrs, cache size, eviction params, poller CPU, backfill delay, partition sizes, cold-load staging slots and buffer size).
- `IpcHandle`: Opaque GPU memory handle (pointer + size).
- `DispatcherError`: 7-variant error enum.
- `CacheKey`: Type alias for `u64`.
- `LookupResult`: 3-variant enum (NotExist, MismatchSize, BlockDevice, MemoryTier).

#### FR-019: Supporting Types - Memory Tier
- `MemoryTierError`: 7-variant error enum.

#### FR-020: Supporting Types - Eviction Policy
- `EvictionHandle`: Opaque handle with pool_id and index.
- `EvictionPolicyError`: 2-variant error enum.
- `PoolId`: Type alias for `u32`.

#### FR-021: Supporting Types - Extent Manager
- `Extent`: Committed extent (key, size, offset).
- `ExtentKey`: Type alias for `u64`.
- `ExtentManagerError`: 5-variant error enum.
- `FormatParams`: 10-field format configuration with defaults.
- `WriteHandle`: Publish/abort handle with auto-abort on drop.

#### FR-022: Supporting Types - GPU Services
- `GpuDeviceInfo`: GPU metadata (index, name, memory, compute capability, PCI bus).
- `GpuIpcHandle`: CUDA IPC memory handle with state tracking (verified, pinned).
- `GpuDmaBuffer`: GPU memory buffer with auto-close on drop.
- `GpuStream`: Opaque CUDA stream handle.

#### FR-023: Supporting Types - Remote
- `RemoteLookupError`: 2-variant error enum.
- `LookupRef`: Zero-copy reference (pointer, size, key).
- `RemoteRequestHandlerError`: 4-variant error enum.

#### FR-024: Supporting Types - Partition Table
- `PartitionInfo`: Partition metadata (index, start LBA, sectors, GUIDs, name).
- `PartitionSpec`: Partition creation spec (type GUID, size, name).
- `PartitionConfig`: Format configuration (sector size, total sectors, ns_id, partitions).
- `PartitionTable`: Resolved partition table (partitions list, sector size).
- `PartitionTableError`: 6-variant error enum.
- `type_guids` module: Well-known Certus partition type GUIDs (CERTUS_METADATA, CERTUS_DATA, CERTUS_EXTERNAL_META).

#### FR-025: Supporting Types - Extended Metadata Store
- `ExtendedMetadataStoreError`: 4-variant error enum (NotFound, StorageError, CapacityExhausted, ValueTooLarge).

#### FR-026: Supporting Types - Dispatch Map
- `DispatchMapError`: 11-variant error enum with reference counting semantics.

#### FR-027: IDispatcher Cold-Load Staging Configuration (feature: spdk)
- The dispatcher SHALL maintain a bounded pool of pre-registered pinned host DRAM staging buffers for SSD→GPU cold loads that cannot obtain a memory-tier slot under pressure, sized by `DispatcherConfig::cold_staging_slots` (buffer count, default 64; 0 disables staging so cold loads fail on a full memory tier) and `DispatcherConfig::cold_staging_buf_bytes` (per-buffer byte capacity, must be ≥ the largest per-block transfer size, default 4 MiB), bounding concurrent cold-read parallelism so a burst cannot exhaust the memory tier.

#### FR-028: IGpuServices Multi-GPU Device Routing
- `IGpuServices` SHALL provide `set_device(device: i32) -> Result<(), String>` to bind the calling OS thread's current CUDA device (CUDA tracks the current device per thread; required before creating a stream or issuing a DMA for a specific GPU) and `device_of_ptr(ptr: *const c_void) -> Result<i32, String>` to return the CUDA device ordinal owning a device pointer via `cudaPointerGetAttributes` (`-1` for a pointer with no device association, e.g. host memory), so DMAs can be routed to a stream on the pointer's own device under multi-GPU / tensor parallelism.

### Non-Functional Requirements

#### NFR-001: Zero Implementation Coupling
- Components depend only on `interfaces` for trait definitions, never on sibling implementation crates.

#### NFR-002: Feature-Gated Compilation
- SPDK-dependent code compiles only with `--features spdk`, allowing the default build to work without SPDK installed.
- GPU-specific code is gated behind `--features gpu`.

#### NFR-003: Thread Safety
- All types that cross thread boundaries implement `Send` (with documented SAFETY justifications for unsafe impls).
- `DmaBuffer`, `GpuIpcHandle`, `GpuDmaBuffer`, `GpuStream`, `LookupResult`, `LookupRef` are `Send + Sync`.
- `Command` is `Send`; `Completion` is `Send + Clone` (Clone enables non-blocking completion delivery on a full ring).

#### NFR-004: Documentation
- All public types and methods have doc comments with runnable examples.
- `cargo doc --no-deps` produces no warnings.

#### NFR-005: Error Handling
- All error types implement `std::error::Error`, `Display`, `Debug`, and `Clone`.
- Error variants carry actionable messages.
- Error conversions via `From` are provided where natural (e.g., `BlockDeviceError` -> `NvmeBlockError`).

#### NFR-006: Deterministic Resource Cleanup
- `DmaBuffer` calls its deallocator on drop (with SPDK-active guard).
- `GpuDmaBuffer` closes the IPC handle on drop.
- `WriteHandle` auto-aborts reservation on drop if not published.

## Key Entities

| Entity | Description |
|--------|-------------|
| `CacheKey` | `u64` identifier for cache entries across all components |
| `ExtentKey` | `u64` identifier for storage extents |
| `PoolId` | `u32` identifier for eviction tracking pools |
| `DmaBuffer` | DMA-safe buffer with pluggable allocator and NUMA awareness |
| `WriteHandle` | Publish-or-abort handle for extent reservations |
| `IpcHandle` | GPU memory reference for DMA transfers |
| `LookupRef` | Zero-copy reference to memory-tier data |
| `ClientChannels` | Channel pair for block device client connections |
| `EvictionHandle` | O(1) handle for eviction policy operations |
| `GpuStream` | Opaque CUDA stream for async GPU operations |

## Dependencies

| Dependency | Purpose |
|------------|---------|
| `component-core` | Core framework traits (`IUnknown`, channels, NUMA) |
| `component-macros` | `define_interface!` procedural macro |
| `spdk-sys` (optional) | Raw FFI bindings for DMA allocation |

## Success Criteria

1. Any component can implement any interface by depending only on `interfaces`.
2. `cargo build` succeeds without the `spdk` feature (no SPDK dependency).
3. `cargo build --features spdk` succeeds with SPDK pre-built.
4. All public items have doc comments; `cargo doc --no-deps` is warning-free.
5. All types that must be `Send`/`Sync` have appropriate implementations.
6. Error types implement `std::error::Error` with human-readable `Display`.
7. Unit tests pass for error display, type equality, and Clone implementations.

## Implementation Notes

- The `define_interface!` macro generates a trait with `IUnknown` as a supertrait, enabling runtime interface discovery via `query_interface`.
- SPDK-gated interfaces are conditionally compiled at the module level (`#[cfg(feature = "spdk")]`).
- `DmaBuffer::Drop` checks the global `SPDK_ENV_ACTIVE` flag to avoid calling into torn-down SPDK infrastructure.
- `WriteHandle` uses `Option<Box<dyn FnOnce()>>` closures for publish/abort, consumed on first call, with drop guard for abort.
- Several interfaces have formally verified properties documented in comments (IDispatchMap: 10 props, IDispatcher: 10 props, IExtentManager: 10 props, IGpuServices: 10 props, IMemoryTier: 10 props).
