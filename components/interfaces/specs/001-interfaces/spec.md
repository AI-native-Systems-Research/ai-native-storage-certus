# Feature Specification: Shared Interface Trait Definitions

**Feature Branch**: `001-interfaces`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice
> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

> **Last Synced 2026-08-07**: FR-006 `track` gained the `semantics: BlockSemantics`
> argument (+ `BlockSemantics`/`SessionId` documented under FR-020); `integrity-check`
> feature and `set_checksum`/`get_checksum` added to the Overview and FR-007;
> `push_async`/`PushCompletion` added to FR-030/FR-033; FR-018 `LookupResult`
> label corrected to 4-variant.

> **Last Synced 2026-09-02**: **FR-014/FR-025 RESOLVED** — the
> `IExtendedMetadataStore` module is no longer orphaned; `src/lib.rs` now declares
> `mod iextended_metadata_store;` and re-exports
> `pub use iextended_metadata_store::{ExtendedMetadataStoreError, IExtendedMetadataStore};`,
> so the trait and its 4-variant error are part of the compiled, exported crate and
> the `extended-metadata-store` consumer resolves. Backfilled from code: FR-008
> (`batch_lookup` now takes `&[(CacheKey, Vec<IpcHandle>)]`; `copy_gpu_to_memory_async`
> now takes `regions: &[IpcHandle]`; added `batch_populate` and `tier_event_stats`);
> FR-018 (added `TierEventStats`); FR-017 (`Command` 12→13 with `FlushSync`,
> `Completion` 11→12 with `FlushDone`, `ReadWriteStats` per-size histograms +
> `IO_SIZE_BUCKETS`); FR-021 (`FormatParams` corrected 10→9 fields); FR-023
> (`LookupConfig` corrected 10→12 fields, added `caller_wait`/
> `connection_teardown_timeout`); new **FR-035** documents `IIpcServer`.
> **Orphaned-module caveat (FR-035):** `IIpcServer` (and `IpcServerConfig`/`IpcError`/
> `IpcMetricsSnapshot`) is defined in `src/iipc.rs` but the `iipc` module is **not yet
> declared/re-exported from `lib.rs`**, so it is not part of the compiled crate. The
> `ipc-component` consumer implements it and would fail to build; this latent break is
> masked only because that crate is excluded from the workspace. See the code-side
> ALIGN task to wire it in (`.specify/sync/align-tasks.md`).

## Overview

The `interfaces` crate provides centralized trait definitions for all Certus component interfaces. It allows components to depend on interface definitions without pulling in implementation crates, enforcing low coupling and enabling independent development. Interfaces are defined using the `define_interface!` procedural macro from `component-macros`, and all components implement `IUnknown` for runtime interface discovery.

The crate has two Cargo features:
- **`spdk`** (optional): Gates SPDK-dependent interfaces and types (`IBlockDevice`, `IBlockDeviceAdmin`, `ISPDKEnv`, `IDispatcher`, `IDispatchMap`, `IMemoryTier`, `IExtentManager`, `IPartitionTable`) and supporting types (`DmaBuffer`, `PciAddress`, etc.).
- **`gpu`** (optional): Reserved for GPU-specific conditional compilation.
- **`integrity-check`** (optional): Adds optional per-entry CRC-32 checksums to `IDispatchMap` (`set_checksum`/`get_checksum`, see FR-007); off by default, with no trait or struct surface change when disabled.

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
- `IMemoryTier` provides a DRAM pool with pluggable eviction (via `IEvictionPolicy`), sharded allocation, and NUMA awareness.

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

**As the** remote lookup component, **I want** interfaces for outbound lookups plus a split RDMA push/accept pair **so that** cache misses can be served from other Certus nodes in a cluster.

**Acceptance Criteria:**
- `IRemoteLookup` provides `initialize`, `batch_lookup`, `join_cluster`, and `leave_cluster`.
- `IRemoteLookupRdmaInitiator` (outbound) RDMA-writes local values into a remote requester's memory; `IRemoteLookupRdmaResponder`/`IRemoteLookupRdmaResponderAdmin` (inbound) accept connections and expose the requester's landing-slot region and control channel. (Supersedes the earlier single `IRemoteRequestHandler` interface and its zero-copy `LookupRef`; see FR-013, FR-030, FR-031.)

### User Story 6 - Extent Management (Priority: P1)

**As the** extent manager component, **I want** a trait defining extent lifecycle operations **so that** the dispatcher can allocate, write, persist, and remove extents on NVMe devices.

**Acceptance Criteria:**
- `IExtentManager` provides `format`, `initialize`, `reserve_extent`, `get_extents`, `for_each_extent`, `remove_extent`, `checkpoint`, `get_instance_id`, `set_checkpoint_interval`, `used_bytes`, `capacity_bytes`, `set_metadata_base_lba`, `set_data_base_lba`, and `data_base_lba`.
- `WriteHandle` implements publish/abort semantics with auto-abort on drop.

### User Story 7 - Peer Discovery and Messaging (Priority: P2)

**As the** remote lookup component (and any other component needing LAN peer discovery), **I want** a factory/handle interface pair over zyre **so that** I can discover cluster peers and exchange group/direct messages without depending on the zyre implementation crate.

**Acceptance Criteria:**
- `IZyre` provides `ping` and `create_node(config: NodeConfig) -> Box<dyn IZyreNode>`.
- `IZyreNode` provides lifecycle (`start`, `stop`), group membership (`join`, `leave`), messaging (`shout`, `whisper`), event reception (`recv`, `try_recv`), and peer/group introspection.
- Discovery supports both UDP beaconing (default) and gossip-based discovery (`GossipConfig`) for clusters spanning subnets.

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
- **Method**: `read_write_stats(&self) -> ReadWriteStats` - Return cumulative per-direction read/write byte, op, and latency counters (zeroed unless built with the `telemetry` feature; monotonic, take deltas across two calls for a window).

#### FR-005: IBlockDeviceAdmin Interface (feature: spdk)
- **Method**: `set_pci_address(&self, addr: PciAddress)` - Set PCI address for controller attachment.
- **Method**: `set_actor_cpu(&self, cpu: usize)` - Pin actor thread to a CPU core.
- **Method**: `initialize(&self) -> Result<(), NvmeBlockError>` - Start the actor thread.
- **Method**: `signal_stop(&self)` - Signal the actor to stop without joining.
- **Method**: `shutdown(&self) -> Result<(), NvmeBlockError>` - Stop actor and join thread.
- **Method**: `detach_controller(&self)` - Detach NVMe controller from SPDK.

#### FR-006: IEvictionPolicy Interface
- **Method**: `create_pool(&self) -> PoolId` - Create a new eviction tracking pool.
- **Method**: `track(&self, pool: PoolId, key: CacheKey, semantics: BlockSemantics) -> Result<EvictionHandle, EvictionPolicyError>` - Register a key for eviction tracking. `semantics` carries per-block metadata (currently `session_id`, see FR-020) that session-aware policies use to group related blocks; pass `BlockSemantics::default()` when not applicable (semantics-free policies such as LRU ignore it).
- **Method**: `touch(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError>` - Record an access, updating the entry's eviction ranking (policy-defined).
- **Method**: `batch_touch(&self, handles: &[EvictionHandle]) -> Result<(), EvictionPolicyError>` - Batched access update.
- **Method**: `remove(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError>` - Stop tracking entry.
- **Method**: `identify_next_to_evict(&self, pool: PoolId) -> Option<CacheKey>` - Select the next key the policy would evict, remove it from tracking, and return it.
- **Method**: `get_eviction_candidates(&self, pool: PoolId, n: usize) -> Vec<CacheKey>` - Return up to N keys the policy would evict next, in eviction order, without removing them.
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
- **Method**: `promote_block_to_memory_tier(&self, key: CacheKey, pointer: *mut u8, size: u32) -> Result<(), DispatchMapError>` - Promote a block-device entry to a memory-tier location **in place**, preserving the entry's eviction handle and all reference counts (works while pinned, unlike remove+recreate); retains `ssd_offset` so the promoted entry stays demotable.
- **Method**: `try_evict_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError>` - Atomically check evictability (MemoryTier state, `ssd_offset: Some(_)`, zero read/write refs) and, if evictable, transition to BlockDevice under a single lock hold.
- **Method**: `set_checksum(&self, key: CacheKey, checksum: u32) -> Result<(), DispatchMapError>` (feature: `integrity-check`) - Record a CRC-32 on the entry so it travels with the index across demote/promote. Error (`KeyNotFound`) if the key is absent. Present only under the `integrity-check` feature.
- **Method**: `get_checksum(&self, key: CacheKey) -> Option<u32>` (feature: `integrity-check`) - Return the recorded CRC-32, or `None` if the key is absent or no checksum was recorded (a stored `0` is treated as unset). Present only under the `integrity-check` feature.

#### FR-008: IDispatcher Interface (feature: spdk)
- **Method**: `initialize(&self, config: DispatcherConfig) -> Result<(), DispatcherError>` - Initialize with PCI addresses and config.
- **Method**: `shutdown(&self) -> Result<(), DispatcherError>` - Shut down, completing in-flight writes.
- **Method**: `lookup(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>` - Look up and DMA-copy to GPU memory.
- **Method**: `lookup_async(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<GpuStream, DispatcherError>` - Async lookup returning CUDA stream.
- **Method**: `batch_lookup(&self, entries: &[(CacheKey, Vec<IpcHandle>)]) -> Vec<Result<(), DispatcherError>>` - Concurrent batch lookup. Each key carries one *or more* GPU destination regions: a block exported as a single coalesced allocation (vLLM ≤0.22, `populate`) has exactly one region; a block split into N per-layer allocations (vLLM 0.23+) has N. The server scatters the one resident DRAM slot back across the N regions in order (region L ← slot + sum of preceding region sizes).
- **Method**: `check(&self, key: CacheKey) -> Result<bool, DispatcherError>` - Check entry existence.
- **Method**: `remove(&self, key: CacheKey) -> Result<(), DispatcherError>` - Remove entry and free resources.
- **Method**: `populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>` - Populate cache from GPU memory.
- **Method**: `reserve_memory(&self, key: CacheKey, size: u32, session_id: u64) -> Result<*mut u8, DispatcherError>` - Reserve memory-tier slot. `session_id` is an opaque per-request identifier (0 = unset) supplied by the client; it carries no allocation semantics and is used only for observability.
- **Method**: `copy_gpu_to_memory_async(&self, key: CacheKey, regions: &[IpcHandle], stream: GpuStream) -> Result<(), DispatcherError>` - DMA copy into a reserved slot. Issues one `cudaMemcpyAsync` per region on the given stream; the N regions are gathered contiguously into the one slot (region L lands at `slot + sum of preceding region sizes`), so a block split into N per-layer GPU allocations is stored as one colocated unit. A single-region slice (`regions.len() == 1`) is the legacy path.
- **Method**: `copy_gpu_to_memory_completed(&self, key: CacheKey, size: u32) -> Result<(), DispatcherError>` - Finalize populated slot.
- **Method**: `release_memory(&self, key: CacheKey) -> Result<(), DispatcherError>` - Release reserved slot (cancellation path).
- **Method**: `touch(&self, key: CacheKey) -> Result<(), DispatcherError>` - Refresh eviction timestamp.
- **Method**: `promote_to_memory_tier(&self, keys: &[CacheKey])` - Promote SSD-resident entries to DRAM.
- **Method**: `clear_memory_tier(&self) -> Result<usize, DispatcherError>` - Evict all memory-tier entries.
- **Method**: `flush_to_ssd(&self) -> Result<usize, DispatcherError>` - Flush pending writes to SSD.
- **Method**: `pin(&self, key: CacheKey) -> Result<(), DispatcherError>` - Take an eviction-protection reference on a cache entry; multiple pins on the same key stack (ref-count increments).
- **Method**: `unpin(&self, key: CacheKey) -> Result<(), DispatcherError>` - Release an eviction-protection reference; the entry becomes evictable again once all pins are released.
- **Method**: `read_write_stats(&self) -> ReadWriteStats` - Return cumulative per-direction SSD read/write byte, op, and latency counters aggregated across all data drives (zeroed unless built with the `telemetry` feature; monotonic).
- **Method**: `batch_populate(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>` - Batch form of `populate`: submits all D2H copies asynchronously, waits for the batch with a single `stream_synchronize`, then registers every entry in the dispatch-map and enqueues SSD write-through. Returns one `Result` per entry, in input order.
- **Method**: `tier_event_stats(&self) -> TierEventStats` - Return cumulative KV-cache tier-movement counters (blocks promoted SSD→DRAM, lookups served to GPU, evictions from the DRAM memory tier and from SSD). Always populated (unconditional, unlike the telemetry-gated `read_write_stats`); monotonic since process start, take deltas across two calls.

#### FR-009: IMemoryTier Interface (feature: spdk)
- **Method**: `initialize(&self, pool_size: usize, numa_node: Option<i32>) -> Result<(), MemoryTierError>` - Initialize pool with NUMA binding.
- **Method**: `insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError>` - Allocate slot.
- **Method**: `get(&self, key: CacheKey) -> Option<(*mut u8, u32)>` - Get pointer, refreshing eviction order.
- **Method**: `peek(&self, key: CacheKey) -> Option<(*mut u8, u32)>` - Get pointer without eviction-order update.
- **Method**: `evict_next(&self) -> Option<CacheKey>` - Evict the eviction policy's next victim.
- **Method**: `evict_next_for_key(&self, key: CacheKey) -> Option<CacheKey>` - Evict the policy's next victim from the target shard.
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
- **Method**: `telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot` - Return a snapshot of cumulative eviction and lock-contention counters (zeroed unless built with the `telemetry` feature).

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

> **Amended** (002 remote-lookup rewrite): `batch_lookup`'s second tuple element
> changed from a GPU `IpcHandle` to a plain `u32` size hint, and an `initialize`
> method plus its `LookupConfig` type were added. `remote-lookup` is now
> CPU/DRAM-only; the GPU-facing zero-copy path moved to
> `IRemoteLookupRdmaInitiator`/`IRemoteLookupRdmaResponder` (see FR-030/FR-031).

- **Method**: `initialize(&self, config: LookupConfig) -> Result<(), RemoteLookupError>` - Configure and activate the component: join the configured zyre group, start peer discovery, and prepare the query/response state machine. Must be called before `batch_lookup`.
- **Method**: `batch_lookup(&self, entries: &[(CacheKey, u32)]) -> Vec<Result<(), RemoteLookupError>>` - Batch lookup from remote nodes; the `u32` is the expected value size (a size hint used to validate the RDMA-written region), not a GPU IPC handle.
- **Method**: `join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError>` - Join a cluster.
- **Method**: `leave_cluster(&self) -> Result<(), RemoteLookupError>` - Leave the cluster.

#### FR-013: Remote Request Handling (RDMA Initiator/Responder Split)

> **SUPERSEDED**: The `IRemoteRequestHandler` interface (`handle_lookup`,
> `handle_check`, `handle_batch_lookup`, `release_lookup`, zero-copy `LookupRef`)
> described in earlier versions of this FR was removed from the codebase and
> does not exist in any form. It has been replaced by a two-interface split
> that separates the outbound (serving) side from the inbound (requesting)
> side of a remote lookup:
> - **`IRemoteLookupRdmaInitiator`** — see **FR-030**. Outbound: RDMA-writes
>   local cache values directly into a remote requester's memory.
> - **`IRemoteLookupRdmaResponder`** / **`IRemoteLookupRdmaResponderAdmin`** —
>   see **FR-031**. Inbound: accepts RDMA connections and exposes the
>   requester's landing-slot region and control channel; a one-sided write
>   path, so this interface carries control traffic only, never data.
>
> Do not re-implement `IRemoteRequestHandler`; this FR number is retained only
> as a pointer to FR-030/FR-031 for historical traceability.

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
- `Command`: 13-variant enum for all NVMe operations (ReadSync, WriteSync, ReadAsync, WriteAsync, WriteZeros, BatchSubmit, AbortOp, NsProbe, NsCreate, NsFormat, NsDelete, ControllerReset, `FlushSync { ns_id: u32 }`).
- `Completion`: 12-variant enum for operation results (ReadDone, WriteDone, WriteZerosDone, AbortAck, Timeout, NsProbeResult, NsCreated, NsFormatted, NsDeleted, ResetDone, `FlushDone { handle, result }`, Error); derives `Clone` (in addition to `Debug`) so the block-device actor can `try_send` a clone of a completion on a full ring without consuming the original, enabling non-blocking completion delivery.
- `ClientChannels`: Channel pair (command_tx, completion_rx).
- `ReadWriteStats`: Cumulative per-direction (read/write) byte, op, and latency-sum counters with `total_ops`/`total_bytes`/`mean_read_latency_ns`/`mean_write_latency_ns` helpers; returned by `IBlockDevice::read_write_stats` and `IDispatcher::read_write_stats` (see FR-004/FR-008). Also carries per-transfer-size histograms `read_size_buckets`/`write_size_buckets: [u64; IO_SIZE_BUCKETS]` with helpers `size_bucket(bytes) -> usize` (power-of-two bucket index, sizes ≥ `2^(IO_SIZE_BUCKETS-1)` clamp into the last bucket), `bucket_lower_bound(idx)`, and `merge_from(&other)` for cross-drive aggregation.
- `IO_SIZE_BUCKETS`: public `usize` const (= 25) — the number of power-of-two transfer-size histogram buckets in `ReadWriteStats`; re-exported from the crate root.

#### FR-018: Supporting Types - Dispatcher
- `DispatcherConfig`: 18-field configuration — `data_pci_addrs`, `max_cache_entries`, `format_on_init`, SSD eviction params (`ssd_eviction_threshold`, `ssd_eviction_low_watermark`, `ssd_eviction_batch_size`, `ssd_eviction_interval_secs`), `poller_base_cpu`, `max_eviction_attempts`, `backfill_delay_ms`, partition sizes (`metadata_partition_size`, `extended_metadata_partition_size`), memory-tier proactive DRAM→SSD demotion params (`memory_tier_eviction_threshold`, `memory_tier_eviction_low_watermark`, `memory_tier_eviction_batch_size`, `memory_tier_eviction_interval_secs` — analogous to the SSD eviction sweep, disabled by default via threshold 0.0), and cold-load staging (`cold_staging_slots`, `cold_staging_buf_bytes`).
- `IpcHandle`: Opaque GPU memory handle (pointer + size).
- `DispatcherError`: 7-variant error enum.
- `CacheKey`: Type alias for `u64`.
- `LookupResult`: 4-variant enum (NotExist, MismatchSize, BlockDevice, MemoryTier).
- `TierEventStats`: `Copy + Default + PartialEq + Eq` cumulative snapshot of KV-cache tier-movement counters, returned by `IDispatcher::tier_event_stats` (see FR-008). 4 `u64` fields: `promotions_to_memory`, `promotions_to_gpu`, `evictions_from_memory`, `evictions_from_ssd`. Monotonic since process start; always populated (not telemetry-gated).

#### FR-019: Supporting Types - Memory Tier
- `MemoryTierError`: 7-variant error enum.
- `MemoryTierTelemetrySnapshot`: `Copy + Default` 3-field cumulative counter snapshot (`evictions`, `write_lock_contentions`, `read_lock_contentions`), returned by `IMemoryTier::telemetry_snapshot` (see FR-009); zeroed unless the `telemetry` feature is enabled.

#### FR-020: Supporting Types - Eviction Policy
- `EvictionHandle`: Opaque handle with pool_id and index.
- `EvictionPolicyError`: 2-variant error enum.
- `PoolId`: Type alias for `u32`.
- `SessionId`: Type alias for `u64` — opaque per-request session identifier (`0` = unset).
- `BlockSemantics`: `Copy + Default` metadata struct carrying `session_id: SessionId`, passed to `IEvictionPolicy::track` (FR-006) so session-aware policies (e.g. session-lists) can group related blocks. The default (`session_id = 0`) is semantics-free; policies like LRU ignore it.

#### FR-021: Supporting Types - Extent Manager
- `Extent`: Committed extent (key, size, offset).
- `ExtentKey`: Type alias for `u64`.
- `ExtentManagerError`: 5-variant error enum.
- `FormatParams`: 9-field format configuration with defaults (`data_disk_size`, `slab_size`, `max_extent_size`, `sector_size`, `region_count`, `metadata_alignment`, `instance_id`, `metadata_disk_ns_id`, `metadata_region_size`).
- `WriteHandle`: Publish/abort handle with auto-abort on drop.

#### FR-022: Supporting Types - GPU Services
- `GpuDeviceInfo`: GPU metadata (index, name, memory, compute capability, PCI bus).
- `GpuIpcHandle`: CUDA IPC memory handle with state tracking (verified, pinned).
- `GpuDmaBuffer`: GPU memory buffer with auto-close on drop.
- `GpuStream`: Opaque CUDA stream handle.

#### FR-023: Supporting Types - Remote
- `RemoteLookupError`: 2-variant error enum (`NotFound`, `TransportError`).
- `LookupConfig`: 12-field configuration for `IRemoteLookup::initialize` — `group`, `quorum_pct`, `phase1_timeout`, `op_deadline`, `caller_wait` (`Option<Duration>`, default `None` — how long `batch_lookup` blocks before returning while the RDMA write may still land), `connection_teardown_timeout` (`Duration`, default 1000 ms), `max_retry_rounds`, `max_keys_per_query`, `bind_ip`, `actor_cpu`, `discovery` (optional `GossipConfig`, see FR-032), `node_endpoint`. Implements `Default`.
- ~~`LookupRef`~~ / ~~`RemoteRequestHandlerError`~~ — **SUPERSEDED**, removed together with `IRemoteRequestHandler` (see FR-013). No replacement type exists: the RDMA split's writes are one-sided (no zero-copy handle is returned to a caller) and its errors are `RemoteLookupRdmaInitiatorError`/`RemoteLookupRdmaResponderError` (FR-033/FR-034).

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

#### FR-029: IZyre / IZyreNode Interfaces (Peer Discovery and Messaging)
- `IZyre` is a **factory** component interface (`define_interface!`, discoverable via `IUnknown`):
  - **Method**: `ping(&self) -> Result<String, ZyreError>` - Check that the zyre subsystem is available and healthy.
  - **Method**: `create_node(&self, config: NodeConfig) -> Result<Box<dyn IZyreNode>, ZyreError>` - Construct a new, un-started node from `config`.
- `IZyreNode` is a **plain trait handle** (deliberately not a `define_interface!` component interface — `Send` but not `Sync`, since the underlying zyre C API is not thread-safe for concurrent access to a single node, and it needs `&mut self` methods), returned by `create_node`:
  - **Method**: `start(&mut self) -> Result<(), ZyreError>` - Begin network discovery and messaging.
  - **Method**: `stop(&mut self)` - Signal departure to peers; enters a draining state (queued events, then a terminal `ZyreEvent::Stop` sentinel, are still deliverable via `recv`/`try_recv`).
  - **Method**: `join(&mut self, group: &str) -> Result<(), ZyreError>` - Join a named group.
  - **Method**: `leave(&mut self, group: &str) -> Result<(), ZyreError>` - Leave a named group.
  - **Method**: `shout(&self, group: &str, data: &[u8]) -> Result<(), ZyreError>` - Send a message to all peers in a group.
  - **Method**: `whisper(&self, peer: &PeerId, data: &[u8]) -> Result<(), ZyreError>` - Send a message directly to a specific peer.
  - **Method**: `recv(&self) -> Result<ZyreEvent, ZyreError>` - Receive the next event (blocking).
  - **Method**: `try_recv(&self) -> Result<Option<ZyreEvent>, ZyreError>` - Receive the next event without blocking.
  - **Method**: `uuid(&self) -> PeerId` - This node's own UUID.
  - **Method**: `name(&self) -> String` - This node's own name.
  - **Method**: `peers(&self) -> Vec<PeerId>` - All known peers.
  - **Method**: `peers_by_group(&self, group: &str) -> Vec<PeerId>` - Peers belonging to a group.
  - **Method**: `own_groups(&self) -> Vec<String>` - Groups this node has joined.
  - **Method**: `peer_groups(&self) -> Vec<String>` - All groups known from any peer.
  - **Method**: `peer_address(&self, peer: &PeerId) -> Option<String>` - Network address of a peer.
  - **Method**: `peer_header_value(&self, peer: &PeerId, key: &str) -> Option<String>` - A specific header value for a peer.

#### FR-030: IRemoteLookupRdmaInitiator Interface (Outbound RDMA Push)
- `IRemoteLookupRdmaInitiator` is the outbound (serving) side of a remote RDMA lookup: given a target host endpoint and a batch of `(key, remote-region)` pairs, it looks each key up in the local memory tier and, when present with a matching size, RDMA-writes the value directly into the remote host's memory (one-sided; the responder's CPU never touches the data).
  - **Method**: `push(&self, endpoint: &str, items: &[(CacheKey, RemoteRegion)]) -> Result<Vec<PushStatus>, RemoteLookupRdmaInitiatorError>` - Ensure a connection to `endpoint` (reusing/repairing as needed), then for each item look up the key locally and RDMA-write it into the remote region if present and size-matched. Returns one `PushStatus` per input item, in order.
  - **Method**: `connect(&self, endpoint: &str) -> Result<(), RemoteLookupRdmaInitiatorError>` - Proactively warm a connection to `endpoint` without writing anything; idempotent, and a failed connect attempt is reported as `Ok(())` with nothing cached (retried on next `connect`/`push`).
  - **Method**: `disconnect(&self, endpoint: &str)` - Tear down the connection to a single host, if any (idempotent).
  - **Method**: `disconnect_all(&self)` - Tear down all connections in the table.
  - **Method**: `set_local_peer_id(&self, peer: PeerId)` - Supply this node's own zyre `PeerId`, stamped into the `rdma_cm` connect `private_data` on every outbound connection so the remote responder can correlate an inbound queue pair to this peer (required for teardown-before-reclaim). Should be called once, before the first `push`.
  - **Method**: `push_async(&self, endpoint: &str, items: &[(CacheKey, RemoteRegion)], on_complete: PushCompletion) -> Result<(), RemoteLookupRdmaInitiatorError>` - Non-blocking form of `push`: queues the batch against `endpoint` and returns before the NIC has read the local buffers. When every write in the batch has completed (on any outcome — success, per-item failure, connection loss, or teardown), `on_complete` is invoked **exactly once** with one `PushStatus` per input item, in request order; on an `Err` return the callback is dropped un-invoked. A submit-queue-full rejection is reported as `UnableToConnect` for every item and may invoke `on_complete` synchronously before returning `Ok(())` (callers must tolerate that reentrancy). The caller must keep the local buffers valid until `on_complete` runs. `push` is a blocking convenience wrapper over this method.

#### FR-031: IRemoteLookupRdmaResponder / IRemoteLookupRdmaResponderAdmin Interfaces (Inbound RDMA Accept)
- These interfaces belong to the **requesting** instance — the passive (accept) counterpart of `IRemoteLookupRdmaInitiator` (FR-030). The responder is an actor owning a dedicated thread running an `rdma_cm` accept loop; because writes are one-sided, this interface carries control traffic only, never data.
- `IRemoteLookupRdmaResponder` (runtime control surface, used by `remote-lookup`):
  - **Method**: `open_control_channel(&self) -> Result<ControlChannel, RemoteLookupRdmaResponderError>` - Open the channel for issuing `ResponderCommand`s and receiving `ResponderEvent`s. Fails if not initialized.
  - **Method**: `local_endpoint(&self) -> Result<Endpoint, RemoteLookupRdmaResponderError>` - Return the bound `{ip, port}` so `remote-lookup` can advertise it in whispers.
  - **Method**: `local_region(&self) -> Result<LocalRegion, RemoteLookupRdmaResponderError>` - Return the pre-registered memory-tier pool region (base address, length, pool-wide `rkey`) so `remote-lookup` can advertise it, paired with each landing-slot address, in its RDMA requests.
- `IRemoteLookupRdmaResponderAdmin` (lifecycle/configuration, driven by the application/mainline, not by `remote-lookup`):
  - **Method**: `set_actor_cpu(&self, cpu: usize)` - Pin the accept-loop thread to `cpu`; must be called before `initialize`.
  - **Method**: `set_bind_ip(&self, ip: String)` - Supply the local RoCE IPv4 the listener binds to; must be called before `initialize`. The responder never auto-detects the address.
  - **Method**: `initialize(&self) -> Result<(), RemoteLookupRdmaResponderError>` - Bind an ephemeral port, register the whole memory-tier pool (via `IMemoryTier::pool_info`) as a `REMOTE_WRITE` memory region, and start the accept loop.
  - **Method**: `signal_stop(&self)` - Signal the accept loop to stop without joining its thread.
  - **Method**: `shutdown(&self) -> Result<(), RemoteLookupRdmaResponderError>` - Stop the accept loop, join its thread, and tear down all connections and the listener.

#### FR-032: Supporting Types - Zyre
- `PeerId`: Newtype wrapper over a UUID `String`; `Debug + Clone + PartialEq + Eq + Hash`; `Display`; `From<String>`/`From<&str>`.
- `ZyreEvent`: 9-variant enum (`Enter`, `Exit`, `Evasive`, `Silent`, `Join`, `Leave`, `Whisper`, `Shout`, `Stop`) with `peer()`/`peer_name()`/`group()` accessors; `Stop` is the terminal end-of-stream sentinel with no peer.
- `NodeConfig`: `#[non_exhaustive]` 9-field configuration for `IZyre::create_node` (`name`, `headers`, `port`, `interface`, `evasive_timeout_ms`, `expired_timeout_ms`, `beacon_interval_ms`, `endpoint`, `gossip`); implements `Default` and a `validate()` used internally by `create_node`.
- `GossipConfig`: `#[non_exhaustive]` 2-field configuration (`bind: Option<String>`, `connect: Vec<String>`) for gossip-based discovery (used instead of UDP beaconing when subnets are crossed); constructed via `GossipConfig::bind(endpoint)` or `GossipConfig::connect(endpoint)`.
- `ZyreError`: 7-variant error enum (`CreateFailed`, `StartFailed`, `NotStarted`, `InvalidConfig`, `SendFailed`, `RecvFailed`, `Stopped`).

#### FR-033: Supporting Types - Remote Lookup RDMA Initiator
- `RemoteRegion`: `Copy` 3-field remote memory descriptor (`addr: u64`, `rkey: u32`, `length: u32`) supplied by the requesting node, identifying where a matching local value may be RDMA-written.
- `PushStatus`: `Copy` 4-variant per-item outcome of `push` (`Success`, `UnableToConnect`, `KeyNotFound`, `SizeMismatch`).
- `PushCompletion`: `Box<dyn FnOnce(Vec<PushStatus>) + Send>` — the completion callback passed to `push_async` (FR-030), invoked exactly once with the per-item `PushStatus` vector when the batch finishes. A callback that releases resources must do so on drop as well as on call, since it is dropped un-invoked when `push_async` returns `Err`.
- `RemoteLookupRdmaInitiatorError`: 2-variant error enum (`NotInitialized`, `InvalidEndpoint`) for method-level (not per-item) failures.

#### FR-034: Supporting Types - Remote Lookup RDMA Responder
- `Endpoint`: 2-field bound listening endpoint (`ip: String`, `port: u16`); `Display` as `"ip:port"`.
- `LocalRegion`: `Copy` 3-field registered pool region (`addr: u64`, `rkey: u32`, `length: usize`); `length` is `usize` (unlike `RemoteRegion::length`) since the whole pool can exceed 4 GiB.
- `ControlChannel`: Channel pair (`command_tx: Sender<ResponderCommand>`, `event_rx: Receiver<ResponderEvent>`) returned by `open_control_channel`.
- `ResponderCommand`: 1-variant enum (`Disconnect { node: PeerId }`) — control commands sent to the responder actor.
- `ResponderEvent`: 3-variant enum (`ConnectionEstablished { node: Option<PeerId> }`, `DisconnectAck { node: PeerId }`, `Error { message: String }`) — events emitted by the responder actor; `DisconnectAck` signals teardown-before-reclaim is complete.
- `RemoteLookupRdmaResponderError`: 6-variant error enum (`NotInitialized`, `AlreadyInitialized`, `Bind`, `Registration`, `ChannelClosed`, `Internal`).

#### FR-035: IIpcServer Interface + Supporting Types (transport-neutral IPC front-end)

> **Orphaned-module caveat**: `IIpcServer` and its supporting types are defined in
> `src/iipc.rs` but the `iipc` module is **not yet declared (`mod`) or re-exported
> (`pub use`) from `src/lib.rs`**, so they are not part of the compiled `interfaces`
> crate today. The consumer `components/ipc-component` does
> `use interfaces::{IIpcServer, IpcError, IpcMetricsSnapshot, IpcServerConfig}` and
> would fail to build; the latent break is masked only because `ipc-component` is
> excluded from the workspace. A code-side task to wire the module in is recorded in
> `.specify/sync/align-tasks.md` (ALIGN-IFACE-001). This FR documents the intended
> contract as it exists in source.

`IIpcServer` is a transport-neutral front-end for the dispatcher: a pluggable
inter-process communication server (`define_interface!`, discoverable via `IUnknown`).
The reference implementation (`ipc-component`) speaks gRPC, but the contract carries no
gRPC/tonic types, so a future shared-memory or RDMA transport can implement the same
interface. The interface is deliberately **un-gated** (no `spdk`/`gpu` types in its
signatures), so mainlines can hold `Arc<dyn IIpcServer>` without the storage feature flags.

- **Method**: `initialize(&self, config: IpcServerConfig) -> Result<(), IpcError>` - Configure the server and prepare transport resources. Must be called before `serve`; a second call returns `IpcError::AlreadyInitialized`.
- **Method**: `serve(&self) -> Result<(), IpcError>` - Run the server, blocking the calling thread until `shutdown` is invoked. The implementation drives its own runtime (e.g. a dedicated OS thread with a tokio runtime), so it is safe to call from inside an async runtime. Returns `IpcError::NotInitialized` if `initialize` has not run.
- **Method**: `shutdown(&self) -> Result<(), IpcError>` - Signal a running `serve` to stop and release transport resources. Idempotent; returns `Ok(())` even when not serving.
- **Method**: `metrics_snapshot(&self) -> IpcMetricsSnapshot` - Read a consistent snapshot of the server's monotonic service counters (pull-based; polled by a telemetry backend).

Supporting types (all `Debug`; `IpcServerConfig`/`IpcError`/`IpcMetricsSnapshot` are `Clone + PartialEq + Eq`):
- `IpcServerConfig`: 4-field, transport-neutral, implements `Default` — `listen_addr: String` (default `"0.0.0.0:50051"`), `tls_cert: Option<String>`, `tls_key: Option<String>` (PEM pair; both or neither; `None` = plaintext), `eviction_channel_capacity: usize` (default 16384).
- `IpcError`: 4-variant error enum (`NotInitialized`, `AlreadyInitialized`, `Config(String)`, `Transport(String)`).
- `IpcMetricsSnapshot`: 5-field `Default` snapshot of cumulative counters — `populates`, `lookup_hits`, `lookup_misses`, `evictions`, `gpu_bytes_transferred` (all `u64`).

### Non-Functional Requirements

#### NFR-001: Zero Implementation Coupling
- Components depend only on `interfaces` for trait definitions, never on sibling implementation crates.

#### NFR-002: Feature-Gated Compilation
- SPDK-dependent code compiles only with `--features spdk`, allowing the default build to work without SPDK installed.
- GPU-specific code is gated behind `--features gpu`.

#### NFR-003: Thread Safety
- All types that cross thread boundaries implement `Send` (with documented SAFETY justifications for unsafe impls).
- `DmaBuffer`, `GpuIpcHandle`, `GpuDmaBuffer`, `GpuStream`, `LookupResult` are `Send + Sync`.
- `Command` is `Send`; `Completion` is `Send + Clone` (Clone enables non-blocking completion delivery on a full ring).
- `IZyreNode` is `Send` but deliberately **not** `Sync` (the underlying zyre C API is not safe for concurrent access to a single node).

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
| `ClientChannels` | Channel pair for block device client connections |
| `EvictionHandle` | O(1) handle for eviction policy operations |
| `GpuStream` | Opaque CUDA stream for async GPU operations |
| `PeerId` | UUID identifier for a zyre network peer |
| `RemoteRegion` / `LocalRegion` | Remote/local RDMA memory descriptors (addr, rkey, length) for one-sided writes |

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
