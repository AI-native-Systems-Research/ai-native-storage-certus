# Feature Specification: Centralized Interface Trait Definitions

**Feature Branch**: `001-interfaces`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `interfaces` crate is the single source of truth for all shared interface trait definitions in the Certus component system. It provides typed contracts (`IBlockDevice`, `IDispatcher`, `IDispatchMap`, `IGpuServices`, `IMemoryTier`, `IEvictionPolicy`, `IExtentManager`, `ISPDKEnv`, `IRemoteLookup`, `IRemoteRequestHandler`, `IPartitionTable`, `IExtendedMetadataStore`, `IGreeter`, `ILogger`) that components depend on instead of depending on each other directly. This keeps coupling low and enables independent development with small LLM context windows.

The crate uses Cargo features to gate hardware-dependent interfaces: the `spdk` feature enables NVMe/DPDK-related traits and types (`ISPDKEnv`, `IBlockDevice`, `IBlockDeviceAdmin`, `IExtentManager`, `IDispatcher`, `IDispatchMap`, `IMemoryTier`, `IPartitionTable`) while the `gpu` feature enables GPU-specific associated types. Interfaces without hardware dependencies (`IGreeter`, `ILogger`, `IGpuServices`, `IEvictionPolicy`, `IRemoteLookup`, `IRemoteRequestHandler`, `IExtendedMetadataStore`) are always available.

## User Scenarios & Testing

### User Story 1 - Component Developer Uses Interface Contracts (Priority: P1)

As a component developer, I want to depend only on the `interfaces` crate for trait definitions, so that I can develop and test my component in isolation without pulling in implementation crates.

**Acceptance Scenarios**:
- Given a new component, when it declares a dependency on `interfaces`, then it can reference any always-available interface trait (e.g., `ILogger`, `IGpuServices`) without additional feature flags.
- Given the `spdk` feature is enabled, when a component references `IBlockDevice` or `IExtentManager`, then it compiles and has access to all SPDK-dependent types.
- Given two components implementing the same interface, when they are swapped at runtime via the component framework, then no changes to the consuming component are required.

### User Story 2 - Dispatcher Orchestrates Cache Operations (Priority: P1)

As the dispatcher component, I want typed interfaces for all sub-components (dispatch-map, memory-tier, extent-manager, block-device, GPU services), so that I can orchestrate cache populate/lookup/eviction flows with compile-time type safety.

**Acceptance Scenarios**:
- Given an `IDispatcher` implementation, when `initialize(config)` is called with valid PCI addresses, then sub-components are created and the dispatcher transitions to initialized state.
- Given a populated cache entry, when `lookup(key, ipc_handle)` is called, then data is DMA-copied to the specified GPU memory address.
- Given the memory-tier is full, when `populate` is called, then LRU eviction terminates within `max_eviction_attempts` iterations.

### User Story 3 - Error Type Consistency (Priority: P2)

As a system integrator, I want all interface error types to implement `Display`, `Debug`, and `std::error::Error`, so that error propagation and logging work uniformly across the component graph.

**Acceptance Scenarios**:
- Given any error enum defined in this crate, when formatted with `Display`, then a human-readable message is produced.
- Given any error enum, when used with the `?` operator, then it propagates correctly through `Result` return types.

## Requirements

### Functional Requirements

#### IGreeter Interface (Always Available)

- **FR-001**: Define `IGreeter` trait with method `greeting_prefix(&self) -> &str` that returns a greeting string prefix.

#### ILogger Interface (Always Available)

- **FR-002**: Define `ILogger` trait with four logging methods: `error(&self, msg: &str)`, `warn(&self, msg: &str)`, `info(&self, msg: &str)`, `debug(&self, msg: &str)`.

#### IEvictionPolicy Interface (Always Available)

- **FR-003**: Define `IEvictionPolicy` trait with methods:
  - `create_pool(&self) -> PoolId` -- create an independent eviction tracking pool.
  - `track(&self, pool: PoolId, key: CacheKey) -> Result<EvictionHandle, EvictionPolicyError>` -- register a key as most-recently-used.
  - `touch(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError>` -- mark entry as most-recently-used (O(1)).
  - `batch_touch(&self, handles: &[EvictionHandle]) -> Result<(), EvictionPolicyError>` -- batch touch for amortized lock overhead.
  - `remove(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError>` -- stop tracking an entry (O(1)).
  - `pop_oldest(&self, pool: PoolId) -> Option<CacheKey>` -- remove and return LRU key (O(1)).
  - `peek_oldest(&self, pool: PoolId, n: usize) -> Vec<CacheKey>` -- return up to N oldest keys without removing (O(n)).
  - `len(&self, pool: PoolId) -> usize` -- return count of tracked entries.
  - `clear_pool(&self, pool: PoolId)` -- remove all entries from a pool.

- **FR-004**: Define `EvictionHandle` as an opaque handle with `pool_id: u32` and `index: u32` fields, supporting `Clone`, `Copy`, `PartialEq`, `Eq`, `Hash`.

- **FR-005**: Define `EvictionPolicyError` enum with variants: `InvalidPool(PoolId)`, `InvalidHandle`.

- **FR-006**: Define `PoolId` as a type alias for `u32`.

#### IGpuServices Interface (Always Available)

- **FR-007**: Define `IGpuServices` trait with methods:
  - `initialize(&self) -> Result<(), String>` -- load CUDA runtime and discover GPUs with compute capability 7.0+.
  - `shutdown(&self) -> Result<(), String>` -- close IPC handles and release GPU resources.
  - `get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String>` -- return info on qualifying GPUs.
  - `deserialize_ipc_handle(&self, base64_payload: &str) -> Result<GpuIpcHandle, String>` -- open a CUDA IPC handle from base64 payload.
  - `verify_memory(&self, handle: &GpuIpcHandle) -> Result<(), String>` -- verify GPU memory is device-type.
  - `pin_memory(&self, handle: &GpuIpcHandle) -> Result<(), String>` -- pin GPU memory for DMA.
  - `unpin_memory(&self, handle: &GpuIpcHandle) -> Result<(), String>` -- unpin previously pinned GPU memory.
  - `create_dma_buffer(&self, handle: GpuIpcHandle) -> Result<GpuDmaBuffer, String>` -- create DMA buffer from verified+pinned handle.
  - `create_stream(&self) -> Result<GpuStream, String>` -- create a CUDA stream.
  - `destroy_stream(&self, stream: GpuStream) -> Result<(), String>` -- destroy a CUDA stream.
  - `stream_query(&self, stream: GpuStream) -> Result<bool, String>` -- non-blocking stream completion check.
  - `stream_synchronize(&self, stream: GpuStream) -> Result<(), String>` -- blocking stream synchronization.

- **FR-008**: When `spdk` feature is enabled, `IGpuServices` additionally provides:
  - `dma_copy_to_host(&self, src, dst: &DmaBuffer, size) -> Result<(), String>` -- sync GPU-to-host copy.
  - `dma_copy_to_device(&self, src: &DmaBuffer, dst, size) -> Result<(), String>` -- sync host-to-GPU copy.
  - `prepare_memory_for_spdk(&self, base64_payload, device_index) -> Result<DmaBuffer, String>` -- one-call IPC handle to DMA buffer.
  - `dma_copy_to_device_async(&self, src: &DmaBuffer, dst, size, stream) -> Result<(), String>` -- async host-to-GPU copy.
  - `memcpy_h2d_async(&self, src, dst, size, stream) -> Result<(), String>` -- async raw ptr host-to-device.
  - `dma_copy_to_host_async(&self, src, dst: &DmaBuffer, size, stream) -> Result<(), String>` -- async GPU-to-host copy.
  - `memcpy_d2h_async(&self, src, dst, size, stream) -> Result<(), String>` -- async raw ptr device-to-host.
  - `allocate_pinned_dma_buffer(&self, size) -> Result<DmaBuffer, String>` -- CUDA-pinned + SPDK-registered buffer.
  - `register_host_memory(&self, ptr, size) -> Result<(), String>` -- register existing memory with CUDA + SPDK.
  - `unregister_host_memory(&self, ptr, size) -> Result<(), String>` -- unregister previously registered memory.

- **FR-009**: Define `GpuDeviceInfo` struct with fields: `device_index: u32`, `name: String`, `memory_bytes: u64`, `compute_major: u32`, `compute_minor: u32`, `pci_bus_id: String`.

- **FR-010**: Define `GpuIpcHandle` struct with state-machine semantics: fresh -> verified -> pinned. Expose `size()`, `as_ptr()`, `is_verified()`, `is_pinned()`, `set_verified()`, `set_pinned()` methods.

- **FR-011**: Define `GpuDmaBuffer` struct owning GPU device memory pointer with RAII cleanup via caller-supplied `free_fn`. Expose `len()`, `is_empty()`, `as_ptr()` methods.

- **FR-012**: Define `GpuStream` as a newtype wrapper `GpuStream(pub *mut c_void)` implementing `Send`, `Sync`, `Clone`, `Copy`.

#### IRemoteLookup Interface (Always Available)

- **FR-013**: Define `IRemoteLookup` trait with methods:
  - `batch_lookup(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), RemoteLookupError>>` -- batch remote cache lookups preserving positional order.
  - `join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError>` -- join a cluster at the specified endpoint.
  - `leave_cluster(&self) -> Result<(), RemoteLookupError>` -- disconnect from cluster.

- **FR-014**: Define `RemoteLookupError` enum with variants: `NotFound`, `TransportError(String)`.

#### IRemoteRequestHandler Interface (Always Available)

- **FR-015**: Define `IRemoteRequestHandler` trait with methods:
  - `handle_lookup(&self, key: CacheKey) -> Result<LookupRef, RemoteRequestHandlerError>` -- zero-copy lookup returning pointer to memory-tier data.
  - `handle_check(&self, key: CacheKey) -> Result<bool, RemoteRequestHandlerError>` -- existence check without acquiring reference.
  - `handle_batch_lookup(&self, keys: &[CacheKey]) -> Vec<Result<LookupRef, RemoteRequestHandlerError>>` -- batch zero-copy lookups.
  - `release_lookup(&self, key: CacheKey)` -- release read reference acquired by lookup.

- **FR-016**: Define `LookupRef` struct with fields: `ptr: *const u8`, `size: u32`, `key: CacheKey`. Must implement `Send` and `Sync`.

- **FR-017**: Define `RemoteRequestHandlerError` enum with variants: `InvalidRequest(String)`, `KeyNotFound(CacheKey)`, `DispatchError(String)`, `NotInitialized(String)`.

#### IExtendedMetadataStore Interface (Always Available)

- **FR-018**: Define `IExtendedMetadataStore` trait with methods:
  - `put(&self, key: &str, value: &[u8]) -> Result<(), ExtendedMetadataStoreError>` -- store a metadata entry.
  - `get(&self, key: &str) -> Result<Vec<u8>, ExtendedMetadataStoreError>` -- retrieve a metadata entry.
  - `delete(&self, key: &str) -> Result<(), ExtendedMetadataStoreError>` -- delete a metadata entry.
  - `iterate_all(&self) -> Result<Vec<(String, Vec<u8>)>, ExtendedMetadataStoreError>` -- snapshot iteration over all entries.
  - `force_flush(&self) -> Result<(), ExtendedMetadataStoreError>` -- flush pending writes to persistent storage.

- **FR-019**: Define `ExtendedMetadataStoreError` enum with variants: `NotFound`, `StorageError(String)`, `CapacityExhausted`, `ValueTooLarge`.

#### IDispatcher Interface (Feature: spdk)

- **FR-020**: Define `IDispatcher` trait with methods:
  - `initialize(&self, config: DispatcherConfig) -> Result<(), DispatcherError>` -- initialize with PCI addresses and cache parameters.
  - `shutdown(&self) -> Result<(), DispatcherError>` -- drain in-flight writes and shut down.
  - `lookup(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>` -- synchronous DMA lookup to GPU memory.
  - `lookup_async(&self, key, ipc_handle) -> Result<GpuStream, DispatcherError>` -- async DMA lookup returning CUDA stream.
  - `batch_lookup(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>` -- concurrent multi-key lookup.
  - `check(&self, key: CacheKey) -> Result<bool, DispatcherError>` -- existence check without data transfer.
  - `remove(&self, key: CacheKey) -> Result<(), DispatcherError>` -- remove entry, blocking on active writers.
  - `populate(&self, key, ipc_handle) -> Result<(), DispatcherError>` -- DMA-copy from GPU into cache with async SSD write-through.
  - `reserve_memory(&self, key, size) -> Result<*mut u8, DispatcherError>` -- reserve DRAM slot without dispatch-map registration.
  - `copy_gpu_to_memory_async(&self, key, ipc_handle, stream) -> Result<(), DispatcherError>` -- async GPU-to-DRAM copy into reserved slot.
  - `copy_gpu_to_memory_completed(&self, key, size) -> Result<(), DispatcherError>` -- finalize slot: register and enqueue write-through.
  - `release_memory(&self, key) -> Result<(), DispatcherError>` -- cancel reserved slot.
  - `touch(&self, key) -> Result<(), DispatcherError>` -- refresh eviction timestamp.
  - `promote_to_memory_tier(&self, keys: &[CacheKey])` -- background SSD-to-DRAM promotion.
  - `clear_memory_tier(&self) -> Result<usize, DispatcherError>` -- evict all memory-tier entries.
  - `flush_to_ssd(&self) -> Result<usize, DispatcherError>` -- block until all write-through completes.

- **FR-021**: Define `DispatcherConfig` struct with fields: `data_pci_addrs`, `max_cache_entries` (default 10000), `format_on_init` (default true), `ssd_eviction_threshold` (default 0.9), `ssd_eviction_low_watermark` (default 0.8), `ssd_eviction_batch_size` (default 64), `ssd_eviction_interval_secs` (default 5), `poller_base_cpu`, `max_eviction_attempts` (default 2048), `backfill_delay_ms` (default 10), `metadata_partition_size` (default 128 MiB), `extended_metadata_partition_size` (default 128 MiB).

- **FR-022**: Define `IpcHandle` struct with fields: `address: *mut u8`, `size: u32`. Must implement `Send`.

- **FR-023**: Define `DispatcherError` enum with variants: `NotInitialized(String)`, `KeyNotFound(CacheKey)`, `AlreadyExists(CacheKey)`, `AllocationFailed(String)`, `IoError(String)`, `Timeout(String)`, `InvalidParameter(String)`.

#### IDispatchMap Interface (Feature: spdk)

- **FR-024**: Define `IDispatchMap` trait with methods:
  - `initialize(&self) -> Result<(), DispatchMapError>` -- recover committed extents from extent-manager.
  - `lookup(&self, key) -> Result<LookupResult, DispatchMapError>` -- blocking lookup with read-ref acquisition.
  - `convert_to_storage(&self, key, offset) -> Result<(), DispatchMapError>` -- transition memory-tier entry to block-device.
  - `take_read(&self, key) -> Result<(), DispatchMapError>` -- acquire read reference.
  - `take_write(&self, key) -> Result<(), DispatchMapError>` -- acquire exclusive write reference.
  - `release_read(&self, key) -> Result<(), DispatchMapError>` -- release read reference.
  - `release_write(&self, key) -> Result<(), DispatchMapError>` -- release write reference.
  - `downgrade_reference(&self, key) -> Result<(), DispatchMapError>` -- atomically convert write to read reference.
  - `remove(&self, key) -> Result<(), DispatchMapError>` -- remove entry (only when unreferenced).
  - `touch(&self, key) -> Result<(), DispatchMapError>` -- update timestamp.
  - `entry_size(&self, key) -> Result<u32, DispatchMapError>` -- query stored size.
  - `oldest_keys(&self, n: usize) -> Vec<CacheKey>` -- return N oldest keys.
  - `create_memory_tier_entry(&self, key, pointer, size) -> Result<(), DispatchMapError>` -- create entry with write reference.
  - `convert_memory_tier_to_block(&self, key) -> Result<(), DispatchMapError>` -- convert when ssd_offset present.
  - `is_evictable(&self, key) -> bool` -- check eviction eligibility.
  - `recover_extent(&self, key, offset, size_blocks) -> Result<(), DispatchMapError>` -- insert recovered extent as BlockDevice entry.

- **FR-025**: Define `CacheKey` as a type alias for `u64`.

- **FR-026**: Define `LookupResult` enum (feature: spdk) with variants: `NotExist`, `MismatchSize`, `BlockDevice { offset: u64 }`, `MemoryTier { pointer: *mut u8, size: u32 }`. Must implement `Send` and `Sync`.

- **FR-027**: Define `DispatchMapError` enum with variants: `KeyNotFound(CacheKey)`, `AlreadyExists(CacheKey)`, `ActiveReferences(CacheKey)`, `Timeout(CacheKey)`, `AllocationFailed(String)`, `InvalidSize`, `NotInitialized(String)`, `RefCountUnderflow(CacheKey)`, `RefCountOverflow(CacheKey)`, `NoWriteReference(CacheKey)`, `InvalidState(String)`.

#### IMemoryTier Interface (Feature: spdk)

- **FR-028**: Define `IMemoryTier` trait with methods:
  - `initialize(&self, pool_size: usize, numa_node: Option<i32>) -> Result<(), MemoryTierError>` -- initialize pool with optional NUMA binding.
  - `insert(&self, key, size) -> Result<*mut u8, MemoryTierError>` -- allocate slot and return pointer.
  - `get(&self, key) -> Option<(*mut u8, u32)>` -- lookup with LRU refresh.
  - `peek(&self, key) -> Option<(*mut u8, u32)>` -- lookup without LRU refresh.
  - `evict_lru(&self) -> Option<CacheKey>` -- evict least-recently-used entry.
  - `evict_lru_for_key(&self, key) -> Option<CacheKey>` -- evict from same shard as key.
  - `oldest_keys(&self, n) -> Vec<CacheKey>` -- peek at N oldest keys.
  - `remove(&self, key) -> Result<(), MemoryTierError>` -- remove specific entry.
  - `touch(&self, key)` -- update LRU position.
  - `batch_touch(&self, keys: &[CacheKey])` -- batch LRU update.
  - `contains(&self, key) -> bool` -- check existence.
  - `capacity(&self) -> usize` -- return total capacity in bytes.
  - `used(&self) -> usize` -- return bytes currently allocated.
  - `pool_info(&self) -> Option<(*mut u8, usize)>` -- return base pointer and size for CUDA registration.
  - `is_dma_capable(&self) -> bool` -- whether pool supports direct NVMe DMA.
  - `clear(&self) -> Result<usize, MemoryTierError>` -- remove all entries.

- **FR-029**: Define `MemoryTierError` enum with variants: `PoolFull`, `KeyNotFound(CacheKey)`, `AlreadyExists(CacheKey)`, `AllocationFailed(String)`, `InvalidSize`, `NotEvictable(CacheKey)`, `NotInitialized(String)`.

#### IExtentManager Interface (Feature: spdk)

- **FR-030**: Define `IExtentManager` trait with methods:
  - `format(&self, params: FormatParams) -> Result<(), ExtentManagerError>` -- format device with validated parameters.
  - `initialize(&self) -> Result<(), ExtentManagerError>` -- recover state from persisted metadata.
  - `reserve_extent(&self, key, size) -> Result<WriteHandle, ExtentManagerError>` -- allocate space with two-phase commit.
  - `get_extents(&self) -> Vec<Extent>` -- return all committed extents.
  - `for_each_extent(&self, cb: &mut dyn FnMut(&Extent))` -- iterate without collecting.
  - `remove_extent(&self, offset) -> Result<(), ExtentManagerError>` -- remove by block offset.
  - `checkpoint(&self) -> Result<(), ExtentManagerError>` -- persist state to metadata device.
  - `get_instance_id(&self) -> Result<u64, ExtentManagerError>` -- return superblock instance ID.
  - `set_checkpoint_interval(&self, interval: Option<Duration>)` -- configure auto-checkpoint.
  - `used_bytes(&self) -> u64` -- return allocated bytes.
  - `capacity_bytes(&self) -> u64` -- return total usable capacity.
  - `set_metadata_base_lba(&self, base_lba: u64)` -- set metadata partition LBA offset.
  - `set_data_base_lba(&self, base_lba: u64)` -- set data partition LBA offset.
  - `data_base_lba(&self) -> u64` -- get data base LBA.

- **FR-031**: Define `Extent` struct with fields: `key: ExtentKey`, `size: u32` (in blocks), `offset: u64`.

- **FR-032**: Define `ExtentKey` as a type alias for `u64`.

- **FR-033**: Define `FormatParams` struct with fields: `data_disk_size`, `slab_size` (default 1 GiB), `max_extent_size` (default 1 GiB), `sector_size` (default 4096), `region_count` (default 16), `metadata_alignment` (default 128 KiB), `instance_id`, `metadata_disk_ns_id` (default 1), `metadata_region_size` (default 128 MiB).

- **FR-034**: Define `WriteHandle` struct implementing two-phase commit: `publish(self) -> Result<Extent, ExtentManagerError>` commits the extent, `abort(self)` cancels. Auto-aborts on drop if not consumed.

- **FR-035**: Define `ExtentManagerError` enum with variants: `CorruptMetadata(String)`, `IoError(String)`, `NotInitialized(String)`, `OffsetNotFound(u64)`, `OutOfSpace`.

#### ISPDKEnv Interface (Feature: spdk)

- **FR-036**: Define `ISPDKEnv` trait with methods:
  - `init(&self) -> Result<(), SpdkEnvError>` -- initialize SPDK/DPDK environment and discover VFIO devices.
  - `fini(&self)` -- tear down SPDK/DPDK environment.
  - `devices(&self) -> Vec<VfioDevice>` -- return discovered devices.
  - `device_count(&self) -> usize` -- return number of discovered devices.
  - `is_initialized(&self) -> bool` -- check initialization state.

- **FR-037**: Define `SpdkEnvError` enum with variants: `VfioNotAvailable(String)`, `PermissionDenied(String)`, `HugepagesNotConfigured(String)`, `AlreadyInitialized(String)`, `InitFailed(String)`, `DeviceProbeFailed(String)`, `DmaAllocationFailed(String)`.

#### IBlockDevice and IBlockDeviceAdmin Interfaces (Feature: spdk)

- **FR-038**: Define `IBlockDevice` trait with methods:
  - `connect_client(&self) -> Result<ClientChannels, NvmeBlockError>` -- create client channel endpoints.
  - `sector_size(&self, ns_id) -> Result<u32, NvmeBlockError>` -- query namespace sector size.
  - `num_sectors(&self, ns_id) -> Result<u64, NvmeBlockError>` -- query namespace sector count.
  - `max_queue_depth(&self) -> u32` -- controller max queue depth.
  - `num_io_queues(&self) -> u32` -- number of I/O queues.
  - `max_transfer_size(&self) -> u32` -- max transfer size in bytes.
  - `block_size(&self) -> u32` -- default namespace block size.
  - `numa_node(&self) -> i32` -- controller NUMA node.
  - `nvme_version(&self) -> String` -- NVMe specification version.
  - `telemetry(&self) -> Result<TelemetrySnapshot, NvmeBlockError>` -- telemetry statistics.

- **FR-039**: Define `IBlockDeviceAdmin` trait with methods:
  - `set_pci_address(&self, addr: PciAddress)` -- configure controller PCI address.
  - `set_actor_cpu(&self, cpu: usize)` -- pin actor thread to CPU core.
  - `initialize(&self) -> Result<(), NvmeBlockError>` -- start actor thread.
  - `signal_stop(&self)` -- signal actor to stop without joining.
  - `shutdown(&self) -> Result<(), NvmeBlockError>` -- stop and join actor thread.
  - `detach_controller(&self)` -- explicitly detach NVMe controller from SPDK.

- **FR-040**: Define `Command` enum with variants: `ReadSync`, `WriteSync`, `ReadAsync`, `WriteAsync`, `WriteZeros`, `BatchSubmit`, `AbortOp`, `NsProbe`, `NsCreate`, `NsFormat`, `NsDelete`, `ControllerReset`.

- **FR-041**: Define `Completion` enum with variants: `ReadDone`, `WriteDone`, `WriteZerosDone`, `AbortAck`, `Timeout`, `NsProbeResult`, `NsCreated`, `NsFormatted`, `NsDeleted`, `ResetDone`, `Error`.

- **FR-042**: Define `ClientChannels` struct with fields: `command_tx: Sender<Command>`, `completion_rx: Receiver<Completion>`.

- **FR-043**: Define `NvmeBlockError` enum with variants: `FeatureNotEnabled(String)`, `NotInitialized(String)`, `Timeout(String)`, `Aborted(String)`, `InvalidNamespace(String)`, `NotSupported(String)`, `BlockDevice(BlockDeviceError)`, `SpdkEnv(SpdkEnvError)`, `LbaOutOfRange(String)`, `ClientDisconnected(String)`. Must implement `From<BlockDeviceError>` and `From<SpdkEnvError>`.

- **FR-044**: Define `TelemetrySnapshot` struct with fields: `total_ops`, `min_latency_ns`, `max_latency_ns`, `mean_latency_ns`, `mean_throughput_mbps`, `elapsed_secs`.

- **FR-045**: Define `OpHandle` as a newtype `OpHandle(pub u64)` implementing `Clone`, `Copy`, `PartialEq`, `Eq`, `Hash`.

- **FR-046**: Define `NamespaceInfo` struct with fields: `ns_id: u32`, `num_sectors: u64`, `sector_size: u32`.

#### IPartitionTable Interface (Feature: spdk)

- **FR-047**: Define `IPartitionTable` trait with methods:
  - `initialize(&self) -> Result<PartitionTable, PartitionTableError>` -- read and validate GPT.
  - `format(&self, config: PartitionConfig) -> Result<PartitionTable, PartitionTableError>` -- write new GPT.
  - `partition_info(&self, index: u32) -> Result<PartitionInfo, PartitionTableError>` -- get partition by index.
  - `num_partitions(&self) -> Result<u32, PartitionTableError>` -- return partition count.

- **FR-048**: Define `PartitionInfo` struct with fields: `index: u32`, `start_lba: u64`, `num_sectors: u64`, `type_guid: [u8; 16]`, `unique_guid: [u8; 16]`, `name: String`.

- **FR-049**: Define `PartitionSpec` struct with fields: `type_guid: [u8; 16]`, `size_bytes: u64` (0 = remaining space), `name: String`.

- **FR-050**: Define `PartitionConfig` struct with fields: `sector_size: u32`, `total_sectors: u64`, `ns_id: u32`, `partitions: Vec<PartitionSpec>`.

- **FR-051**: Define `PartitionTable` struct with fields: `partitions: Vec<PartitionInfo>`, `sector_size: u32`.

- **FR-052**: Define `PartitionTableError` enum with variants: `NoPartitionTable(String)`, `CorruptTable(String)`, `InvalidPartition(String)`, `IoError(String)`, `LayoutError(String)`, `NotInitialized(String)`.

- **FR-053**: Define `type_guids` module with constants: `CERTUS_METADATA`, `CERTUS_DATA`, `CERTUS_EXTERNAL_META` (16-byte mixed-endian GUID arrays).

#### SPDK Shared Types (Feature: spdk)

- **FR-054**: Define `DmaBuffer` struct backed by SPDK hugepage memory or external allocator, with RAII deallocation via `free_fn`. Must implement `Send`, `Sync`, `Deref<Target=[u8]>`, `DerefMut`. Expose: `new(size, align, numa_node)`, `from_raw(ptr, len, free_fn, numa_node)`, `len()`, `is_empty()`, `as_ptr()`, `as_slice()`, `as_mut_slice()`, `numa_node()`, `set_numa_node()`, `metadata()`, `metadata_mut()`.

- **FR-055**: `DmaBuffer::new` must reject size == 0 with `DmaAllocationFailed` error.

- **FR-056**: `DmaBuffer::from_raw` must reject null pointers and size == 0.

- **FR-057**: `DmaBuffer::drop` must skip deallocation if `is_spdk_env_active()` returns false (prevents crash after SPDK teardown).

- **FR-058**: Define `PciAddress` struct with fields: `domain: u32`, `bus: u8`, `dev: u8`, `func: u8`. Display in `DDDD:BB:DD.F` notation.

- **FR-059**: Define `PciId` struct with fields: `class_id: u32`, `vendor_id: u16`, `device_id: u16`, `subvendor_id: u16`, `subdevice_id: u16`.

- **FR-060**: Define `VfioDevice` struct with fields: `address: PciAddress`, `id: PciId`, `numa_node: i32`, `device_type: String`.

- **FR-061**: Define `BlockDeviceError` enum with variants: `NotOpen(String)`, `AlreadyOpen(String)`, `ProbeFailure(String)`, `NamespaceNotFound(String)`, `QpairAllocationFailed(String)`, `ReadFailed(String)`, `WriteFailed(String)`, `BufferSizeMismatch(String)`, `DmaAllocationFailed(String)`, `EnvNotInitialized(String)`.

- **FR-062**: Provide process-global `set_spdk_env_active(bool)` and `is_spdk_env_active() -> bool` functions using `AtomicBool` with Release/Acquire ordering.

- **FR-063**: Define `DmaAllocFn` as `Arc<dyn Fn(usize, usize, Option<i32>) -> Result<DmaBuffer, String> + Send + Sync>` for pluggable DMA allocators.

### Non-Functional Requirements

- **NFR-001**: All interface traits must be defined using the `define_interface!` macro from `component-macros`, ensuring automatic `IUnknown` integration and runtime interface discovery.

- **NFR-002**: All error enums must implement `fmt::Display`, `fmt::Debug`, and `std::error::Error`.

- **NFR-003**: Types containing raw pointers that are safe to share across threads must explicitly implement `Send` and/or `Sync` with `// SAFETY:` justification comments.

- **NFR-004**: SPDK-dependent interfaces and types must be gated behind `#[cfg(feature = "spdk")]` to allow compilation without SPDK.

- **NFR-005**: GPU-specific types must be gated behind `#[cfg(feature = "gpu")]`.

- **NFR-006**: Public APIs must include doc comments with runnable examples where practical (`/// # Examples` blocks).

- **NFR-007**: The crate must compile with `cargo clippy -- -D warnings` (zero warnings).

- **NFR-008**: The crate must produce warning-free documentation with `cargo doc --no-deps`.

- **NFR-009**: All default-available types (those without feature gates) must compile on any Linux platform without hardware dependencies.

- **NFR-010**: Interface trait methods must use `&self` receivers to support concurrent access through `Arc` wrappers.

## Key Entities

| Entity | Description |
|--------|-------------|
| `CacheKey` | Type alias `u64` -- universal key for cache entries across all components |
| `ExtentKey` | Type alias `u64` -- identifies extents in the extent manager |
| `PoolId` | Type alias `u32` -- identifies eviction tracking pools |
| `DmaBuffer` | SPDK hugepage-backed or externally-backed I/O buffer with RAII deallocation |
| `GpuDmaBuffer` | GPU device memory buffer with IPC handle cleanup on drop |
| `GpuIpcHandle` | State-machine handle for GPU memory (fresh -> verified -> pinned) |
| `GpuStream` | Opaque CUDA stream handle for async operations |
| `IpcHandle` | Raw pointer + size pair for GPU DMA targets |
| `LookupRef` | Zero-copy reference to memory-tier data with release protocol |
| `WriteHandle` | Two-phase commit handle for extent reservation (publish or auto-abort) |
| `EvictionHandle` | Opaque O(1) handle for eviction policy touch/remove |
| `PartitionInfo` | GPT partition metadata snapshot |
| `DispatcherConfig` | Full configuration for dispatcher initialization |
| `FormatParams` | Extent manager format-time parameters |
| `TelemetrySnapshot` | Point-in-time I/O performance statistics |

## Dependencies

| Dependency | Purpose |
|------------|---------|
| `component-core` | Core framework traits (IUnknown), channel types (Sender, Receiver), NUMA utilities |
| `component-macros` | `define_interface!` macro for trait generation with IUnknown integration |
| `spdk-sys` (optional, feature = "spdk") | Raw FFI bindings to SPDK C libraries for DmaBuffer allocation |

## Success Criteria

1. All interface traits compile without the `spdk` feature on any Linux system.
2. All interface traits compile with the `spdk` feature when SPDK is available.
3. Error types produce clear, actionable messages when displayed.
4. No component implementation depends on another component's internals -- only on traits and types from this crate.
5. All `unsafe impl Send/Sync` declarations have corresponding safety justification.
6. `cargo test -p interfaces` passes with all unit tests for error display formatting and type construction.
7. `cargo clippy -p interfaces -- -D warnings` produces zero diagnostics.
8. `cargo doc -p interfaces --no-deps` produces zero warnings.

## Implementation Notes

- The `define_interface!` macro generates a trait that extends `IUnknown`, enabling runtime interface discovery via `query_interface()`. This is the cornerstone of the COM-inspired component model.
- `IpcHandle` implements `Send` despite containing a raw pointer because the GPU memory is accessible cross-thread via DMA engine. The caller guarantees pointer validity for the operation duration.
- `LookupResult::MemoryTier` implements `Send`/`Sync` because the memory-tier pool is a long-lived mmap'd region protected by the dispatch-map's mutex.
- `DmaBuffer::drop` checks `is_spdk_env_active()` to avoid calling into torn-down SPDK C libraries, which would cause a segfault. The OS reclaims memory on process exit.
- `WriteHandle` uses `Option<Box<dyn FnOnce()>>` for publish/abort closures, enabling exactly-once semantics enforced by Rust's move semantics. The `Drop` impl auto-aborts if neither `publish()` nor `abort()` was called.
- Formally verified properties are documented in comments within each interface file, referencing Creusot proofs in the corresponding component's `verif/` directory.
