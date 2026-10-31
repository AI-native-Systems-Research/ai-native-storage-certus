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
> label corrected to 4-variant. FR-014/FR-025 `IExtendedMetadataStore` was then
> an orphaned module (deferred). *(Resolved 2026-08-20: now declared and
> re-exported, ungated, at `src/lib.rs:83,105`; see `.specify/sync/align-tasks.md`.)*

> **Last Synced 2026-10-21** (branch `fix/dispatcher-remove-clear-mt`, approved by
> Cornel): backfilled FR-017 (`Command`/`Completion` counts, `ReadWriteStats`
> histograms), FR-018 (`DispatcherConfig` 19 fields, `TierEventStats`), FR-008 / US2
> (method inventory, `copy_gpu_to_memory_async` regions, `clear_memory_tier` /
> `flush_to_ssd` result contracts), FR-021, FR-023, Overview/FR-009 gating, NFR-002 and
> the Implementation Notes. See `.specify/sync/apply-report.md`.

> **Last Synced 2026-10-30** (branch `specsync-a`, optimizer session `s20261029-094947`
> merged onto unstable): backfilled FR-011 (`IGpuServices::memcpy_batch_async`, added in
> `4941320a`) and the Implementation Notes line citation; `plan.md` method counts
> recounted. The clippy commit `55a4ddd4` (doc comments, `#[must_use]`, SAFETY comments,
> `GpuDmaBuffer` `Debug` via `finish_non_exhaustive()`, `# Errors`/`# Panics` on
> `WriteHandle::publish` and `IZyreNode`) contradicts no requirement. See
> `.specify/sync/apply-report.md`.

## Overview

The `interfaces` crate provides centralized trait definitions for all Certus component interfaces. It allows components to depend on interface definitions without pulling in implementation crates, enforcing low coupling and enabling independent development. Interfaces are defined using the `define_interface!` procedural macro from `component-macros`, and all components implement `IUnknown` for runtime interface discovery.

The crate has three Cargo features:
- **`spdk`** (optional): Gates SPDK-dependent interfaces and types (`IBlockDevice`, `IBlockDeviceAdmin`, `ISPDKEnv`, `IDispatcher`, `IExtentManager`, `IPartitionTable`) and supporting types (`DmaBuffer`, `PciAddress`, etc.). Interfaces that merely carry opaque pointers or plain data are **not** gated even where their implementations need SPDK — `IMemoryTier`, `IDispatchMap`/`LookupResult`, and `ITransferEndpoint`/`ITransferEndpointAdmin` (FR-035) — so a component can consume them as a default workspace member without `spdk-sys`.
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
- `IDispatcher` provides `initialize`, `shutdown`, `lookup`, `lookup_async`, `batch_lookup`, `check`, `remove`, `populate`, `batch_populate`, `reserve_memory`, `copy_gpu_to_memory_async`, `copy_gpu_to_memory_completed`, `release_memory`, `pin`, `unpin`, `touch`, `promote_to_memory_tier`, `clear_memory_tier`, `clear_all`, `flush_to_ssd`, `schedule_write_through`, `read_write_stats`, `tier_event_stats`, `read_to_bounce`, and `release_bounce` methods.
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
- `IRemoteLookup` provides `initialize`, `batch_lookup`, `join_cluster`, `leave_cluster`, and `serve_stats` *(the last added 2026-09-29)*.
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

#### FR-007: IDispatchMap Interface (ungated; `set_checksum`/`get_checksum` feature: `integrity-check`)
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
- **Method**: `batch_lookup(&self, entries: &[(CacheKey, Vec<IpcHandle>)]) -> Vec<LookupOutcome>` *(Sync 2026-10-01 — CODE CHANGE, served_by Phase 2)* - Concurrent batch lookup. Returns one `LookupOutcome` per requested entry, in order, each carrying **how** the key was served as well as the result. Widened from `Vec<Result<(), DispatcherError>>`: the tier was previously lost at this boundary, so no consumer could distinguish a DRAM hit from an SSD read or a peer fetch.
- **Method**: `check(&self, key: CacheKey) -> Result<bool, DispatcherError>` - Check entry existence.
- **Method**: `remove(&self, key: CacheKey) -> Result<(), DispatcherError>` - Remove entry and free resources.
- **Method**: `populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError>` - Populate cache from GPU memory.
- **Method**: `batch_populate(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>` - Semantically `populate` per entry, but all D2H copies are submitted asynchronously and a single `stream_synchronize` covers the batch. Returns one `Result` per entry, in order.
- **Method**: `reserve_memory(&self, key: CacheKey, size: u32, session_id: u64, deadline: Option<std::time::Instant>) -> Result<*mut u8, DispatcherError>` - Reserve memory-tier slot. `session_id` is an opaque per-request identifier (0 = unset) supplied by the client; it carries no allocation semantics and is used only for observability. `deadline` bounds how long a full-tier reservation may backpressure (retry LRU eviction) before returning `AllocationFailed`: `None` uses the dispatcher's own `store_backpressure_ms` budget measured from the start of this call (legacy per-call behavior); `Some(instant)` retries only until `instant` — a deadline the caller shares across a batch of reserves so the batch's *total* backpressure is bounded (rather than paying the full budget per key), then makes one best-effort attempt and fails fast. *(Sync 2026-09-15 — code authoritative, commit `bec6c6ec`: added the `deadline` parameter so a multi-key OP_RESERVE batch does not overrun the client's request deadline.)*
- **Method**: `copy_gpu_to_memory_async(&self, key: CacheKey, regions: &[IpcHandle], stream: GpuStream) -> Result<(), DispatcherError>` - DMA copy into reserved slot. The N regions are gathered contiguously into the one slot (region L lands at `slot + sum of preceding region sizes`), so a block split into N per-layer GPU allocations (vLLM 0.23+) is stored as one unit; `regions.len() == 1` is the legacy single-allocation path.
- **Method**: `copy_gpu_to_memory_completed(&self, key: CacheKey, size: u32) -> Result<(), DispatcherError>` - Finalize populated slot.
- **Method**: `release_memory(&self, key: CacheKey) -> Result<(), DispatcherError>` - Release reserved slot (cancellation path).
- **Method**: `pin(&self, key: CacheKey) -> Result<(), DispatcherError>` - Take an eviction-protection reference on a cache entry; multiple pins on the same key stack (ref-count increments).
- **Method**: `unpin(&self, key: CacheKey) -> Result<(), DispatcherError>` - Release an eviction-protection reference; the entry becomes evictable again once all pins are released.
- **Method**: `touch(&self, key: CacheKey) -> Result<(), DispatcherError>` - Refresh eviction timestamp.
- **Method**: `promote_to_memory_tier(&self, keys: &[CacheKey])` - Promote SSD-resident entries to DRAM.
- **Method**: `clear_memory_tier(&self) -> Result<usize, DispatcherError>` - Empty the memory tier: entries with an SSD copy are demoted to BlockDevice, entries without one are removed. A **pinned** entry (a lookup, a client `pin`, or the pending write-through's own reference) keeps its slot and is retried; if pinned entries remain when the implementation's bounded wait expires, returns `Timeout` and leaves them resident. `Ok(n)` means the tier was emptied, and `n` counts only entries actually demoted or removed. *(Sync 2026-10-21 — CODE CHANGE, repair `cccfb328e`; behaviour in dispatcher spec 001 FR-038.)*
- **Method**: `clear_all(&self) -> Result<ClearAll, DispatcherError>` - Remove every entry from both tiers, freeing slots and extents, reporting each as an `EvictionReason::Removed` event. A pinned entry is kept; `ClearAll { removed, still_pinned }` says how many of each, and a pin outlasting the bounded wait is not an error. *(2026-10-24 — CODE CHANGE; behaviour in dispatcher spec 001 FR-063.)*
- **Struct**: `ClearAll { removed: u64, still_pinned: u64 }` - What `clear_all` did.
- **Method**: `flush_to_ssd(&self) -> Result<usize, DispatcherError>` - Block until every queued write-through has run. Returns `IoError` if a write-through failed for an entry that is still in the memory tier (that entry has no SSD copy, so a later `clear_memory_tier` would drop its only copy); otherwise returns the number of memory-tier entries that now have a valid SSD offset. *(Sync 2026-10-21 — CODE CHANGE, repair `9bd231d85` / DIS-FLUSH-REPORTS-FAILED-WRITE; behaviour in dispatcher spec 001 FR-056.)*
- **Method**: `schedule_write_through(&self, key: CacheKey, size: u32) -> Result<(), DispatcherError>` - Non-blocking, best-effort background write-through; contract in FR-031a.
- **Method**: `read_write_stats(&self) -> ReadWriteStats` - Return cumulative per-direction SSD read/write byte, op, and latency counters aggregated across all data drives (zeroed unless built with the `telemetry` feature; monotonic).
- **Method**: `tier_event_stats(&self) -> TierEventStats` - Cumulative KV-cache tier-movement counters (see FR-018, FR-018a); always populated (not telemetry-gated); implementations that do no tiering return zeroes.
- **Method**: `read_to_bounce(&self, key: CacheKey, offset: u64, len: u32) -> Result<BounceBuffer, BounceError>` - Read the block-tier entry `key`, `len` bytes at extent `offset`, into a fresh bounce buffer (FR-009) for serving a peer, from the drive the write path's placement hash puts it on. The caller MUST hold a pin on `key` (its `IDispatchMap::lookup` `BlockDevice { offset }` answer) for the whole call; this method takes and releases none. Never promotes and never touches the cache, so serving a peer cannot evict. Blocks until the read completes — deliberately no device timeout, since freeing a buffer the drive may still write is unsafe — and holds no dispatcher-wide lock meanwhile, so a hung read does not hold up `shutdown`. Errors: `NoSpace` (retry), `NeverFits` (never satisfiable: larger than the reserve, no data drives, or a drive block that does not divide 4 KiB), `Io` (read failed; buffer freed, or deliberately leaked if the drive may still write it), `NotInitialized`.
- **Method**: `release_bounce(&self, buf: BounceBuffer)` - Free a buffer from `read_to_bounce` once nothing can still read it (its write has completed).

#### FR-009: IMemoryTier Interface (ungated)
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
- **Method**: `capacity(&self) -> usize` - Return the bytes the cache may use: the pool less the bounce reserve (zero before `initialize`).
- **Method**: `used(&self) -> usize` - Return bytes allocated.
- **Method**: `pool_info(&self) -> Option<(*mut u8, usize)>` - Return the whole pool's base pointer and size, bounce reserve included, for CUDA host and RDMA registration.
- **Method**: `is_dma_capable(&self) -> bool` - Check if pool supports direct NVMe DMA.
- **Method**: `clear(&self) -> Result<usize, MemoryTierError>` - Remove all entries.
- **Method**: `telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot` - Return a snapshot of cumulative eviction and lock-contention counters (zeroed unless built with the `telemetry` feature).
- **Method**: `set_bounce_reserve(&self, bytes: usize) -> Result<(), MemoryTierError>` - Override the bounce-reserve size (default `DEFAULT_BOUNCE_RESERVE`, FR-019). Only before `initialize`, which takes `min(bytes, pool_size / 8)`, in whole 4 KiB pages, from the top of the pool; afterwards an error. `0` means no reserve.
- **Method**: `bounce_alloc(&self, len: u32) -> Result<BounceBuffer, BounceError>` - Allocate `len` bytes (rounded up to 4 KiB) of the reserve, which lies inside the pool but outside the cache, so a buffer is DMA-able and covered by any whole-pool registration, and taking one never evicts. `NoSpace` if it does not fit now; `NeverFits` if it never can (larger than the reserve, zero, no reserve, or a pool that is not DMA-capable); `NotInitialized` before `initialize`.
- **Method**: `bounce_free(&self, buf: BounceBuffer)` - Return a buffer, with the `len` it was allocated with; only once no device or NIC can still access its bytes.
- **Method**: `bounce_usage(&self) -> (usize, usize)` - Reserve size and bytes allocated now, both after 4 KiB rounding.

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
- **Method**: `memcpy_batch_async(&self, dsts: &[*mut c_void], srcs: &[*const c_void], sizes: &[usize], stream: GpuStream) -> Result<(), String>` (feature: spdk) *(Sync 2026-10-30 — CODE CHANGE, commit `4941320a`)* - Submit a batch of raw-pointer copies in one call (`cudaMemcpyBatchAsync`): entry `i` copies `sizes[i]` bytes from `srcs[i]` to `dsts[i]`, and the direction (H2D or D2H) comes from the pointers. The batch as a whole runs in stream order on `stream`, but there is **no order among its entries**, so the entries MUST NOT overlap or depend on each other. Caller contract: every pointer is valid for its size until the stream is synchronized, host pointers are CUDA-pinned, and `stream` is a created stream — the legacy NULL stream is not accepted by the batch API, so a caller with no created stream uses the per-copy `memcpy_h2d_async`/`memcpy_d2h_async` instead. Returns `Err` if GPU support is not compiled in, the component is not initialized, the three slices differ in length, or the CUDA call fails; an empty batch is `Ok` with no CUDA call. Used by the dispatcher to submit one block's piece copies at once (dispatcher spec 001 FR-056).
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
- **Method**: `serve_stats(&self) -> RemoteServeStats` *(Sync 2026-09-29 — CODE CHANGE)* - Cumulative snapshot of what this node has served **to its peers**: keys served, and of those, keys read from this node's own block tier to serve them. Responder-side, the opposite direction from `TierEventStats::remote_lookup_hits` (which is what this node obtained *from* peers). Readable before `initialize` (zeroes), never reset by reading. Specified in `components/remote-lookup/specs/002-remote-lookup-rdma` FR-035/FR-036.

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
- `Command`: 13-variant enum for all NVMe operations (ReadSync, WriteSync, ReadAsync, WriteAsync, WriteZeros, BatchSubmit, AbortOp, NsProbe, NsCreate, NsFormat, NsDelete, ControllerReset, FlushSync). *(Sync 2026-10-21 — backfilled: `FlushSync { ns_id }` forces the namespace's volatile write cache to media, added in `e56a8b4e6` for the extent-manager `volatile_write_cache` feature; behaviour in block-device-spdk-nvme FR-031.)*
- `Completion`: 12-variant enum for operation results (ReadDone, WriteDone, WriteZerosDone, AbortAck, Timeout, NsProbeResult, NsCreated, NsFormatted, NsDeleted, ResetDone, FlushDone, Error); derives `Clone` (in addition to `Debug`) so the block-device actor can `try_send` a clone of a completion on a full ring without consuming the original, enabling non-blocking completion delivery.
- `ClientChannels`: Channel pair (command_tx, completion_rx).
- `ReadWriteStats`: Cumulative per-direction (read/write) byte, op, and latency-sum counters plus per-direction power-of-two IO-size histograms (`read_size_buckets`/`write_size_buckets`, `IO_SIZE_BUCKETS` = 25 buckets; `size_bucket`/`bucket_lower_bound` map sizes to buckets), with `total_ops`/`total_bytes`/`mean_read_latency_ns`/`mean_write_latency_ns` helpers and `merge_from` for aggregating across drives; returned by `IBlockDevice::read_write_stats` and `IDispatcher::read_write_stats` (see FR-004/FR-008).

#### FR-018: Supporting Types - Dispatcher
- `DispatcherConfig`: 19-field configuration — `data_pci_addrs`, `max_cache_entries`, `format_on_init`, SSD eviction params (`ssd_eviction_threshold`, `ssd_eviction_low_watermark`, `ssd_eviction_batch_size`, `ssd_eviction_interval_secs`), `poller_base_cpu`, `max_eviction_attempts`, `store_backpressure_ms` (store-path backpressure budget, default 5000 ms, 0 = fail fast; see `reserve_memory` in FR-008), `backfill_delay_ms`, partition sizes (`metadata_partition_size`, `extended_metadata_partition_size`), memory-tier proactive DRAM→SSD demotion params (`memory_tier_eviction_threshold`, `memory_tier_eviction_low_watermark`, `memory_tier_eviction_batch_size`, `memory_tier_eviction_interval_secs` — analogous to the SSD eviction sweep, disabled by default via threshold 0.0), and cold-load staging (`cold_staging_slots`, `cold_staging_buf_bytes`).
- `IpcHandle`: Opaque GPU memory handle (pointer + size).
- `DispatcherError`: 7-variant error enum.
- `CacheKey`: Type alias for `u64`.
- `LookupResult`: 4-variant enum (NotExist, MismatchSize, BlockDevice, MemoryTier).
- `TierEventStats` *(Sync 2026-10-21 — backfilled)*: `Copy + Default` 16-field snapshot returned by `IDispatcher::tier_event_stats` — promotions (`promotions_to_memory`, `promotions_to_gpu`), the route partition (`lookup_hits_dram`, `lookup_hits_ssd`, `remote_lookup_hits`, `remote_lookup_misses`), evictions (`evictions_from_memory`, `evictions_from_ssd`), store refusals (`store_backpressure_events`, `store_drops_on_full`, `store_already_resident`; see FR-018a), clean-eviction diagnostics (`evictions_blocked_by_pin`, `eviction_scans_exhausted`, `evictions_blocked_unpersisted`) and the two **gauges** `oldest_sampled`/`oldest_persisted`. All other fields are monotonic since process start.
- `EvictionEvent` / `EvictionReason` *(moved here 2026-10-02)*: `EvictionEvent { key, reason }` with `EvictionReason` being `Demoted` (to the block device) or `Removed`. Emitted best-effort on every memory-tier eviction over a bounded single-subscriber channel, with drop-and-count backpressure; the behavioural contract lives with the dispatchers (dispatcher spec 001 FR-042/FR-050, dispatcher-p2p spec 001 FR-017).
  - **Defined here, and only here, because two components emit it.** Both `dispatcher` and `dispatcher-p2p` publish these events, and a type defined in one of them cannot be named by the other without one dispatcher depending on the other — which is what prevented the `full-p2p` profile from building at all. A shared vocabulary type belongs in `interfaces` for the same reason `CacheKey` and `LookupResult` do.

#### FR-018a: Store-Refusal Accounting *(New 2026-10-03)*
- `TierEventStats` MUST distinguish the reasons a reserve was refused, because the shm-queue wire reports every reserve error as the same flat zero (`op_reserve` writes `0` on any `Err`) and the reasons mean opposite things:
  - `store_backpressure_events` — retries taken while the tier was momentarily full.
  - `store_drops_on_full` — the backpressure budget elapsed and the store was dropped.
  - `store_already_resident` — `mt.insert` returned `AlreadyExists`: another writer, or a remote fetch's `publish_success`, got there first. **Not a capacity refusal.**
- `store_already_resident` MUST be counted rather than inferred, because that path bypasses the backpressure retry and so bumps neither of the other two. *(Sync 2026-10-21 — CODE CHANGE, repair DIS-STORE-ALREADY-RESIDENT-COUNTS-STORES-ONLY: it is counted only in `reserve_memory`, the store path. A load that loses the insert race to a concurrent promotion is a hit and is not counted.)* A measured run showed 7.28% of stores refused while `eviction_scans_exhausted` was 0 — eviction never failed once — which no combination of the pre-existing counters could explain.
- All three MUST be exposed by the server's metrics endpoint. They existed in this struct and were computed by the dispatcher, but were published nowhere, so the question above could not be answered from a scrape.
- **Known asymmetry, recorded not resolved**: on the load path the identical `AlreadyExists` from `mt.insert` is treated as a *hit* (`serve_concurrently_promoted`), while on the store path it is surfaced as a failure. Whether the wire should keep conflating "already cached" with "cache full" is a protocol question; these counters measure it without settling it.

#### FR-019: Supporting Types - Memory Tier
- `MemoryTierError`: 7-variant error enum.
- `MemoryTierTelemetrySnapshot`: `Copy + Default` 3-field cumulative counter snapshot (`evictions`, `write_lock_contentions`, `read_lock_contentions`), returned by `IMemoryTier::telemetry_snapshot` (see FR-009); zeroed unless the `telemetry` feature is enabled.
- `BounceBuffer`: `Copy` 2-field range of the bounce reserve (`ptr: *mut u8`, `len: u32` — the length requested, which must be passed back unchanged when freed); `Send` but not `Sync` (NFR-003).
- `BounceError`: 4-variant error enum (`NoSpace`, `NeverFits(String)`, `Io(String)`, `NotInitialized(String)`) with `is_transient()` true only for `NoSpace`. Shared by `IMemoryTier::bounce_alloc` and `IDispatcher::read_to_bounce`.
- `DEFAULT_BOUNCE_RESERVE`: `usize` constant, 32 MiB — four concurrent device reads (the transfer endpoint's default `block_readers`, FR-036) of up to 8 MiB, the per-worker 64-token KV block `DispatcherConfig::cold_staging_buf_bytes` already provides for (FR-027). Taken at `initialize` unless overridden by `set_bounce_reserve`, capped at an eighth of the pool.

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
- `FormatParams`: 9-field format configuration with defaults (`data_disk_size`, `slab_size`, `max_extent_size`, `sector_size`, `region_count`, `metadata_alignment`, `instance_id`, `metadata_disk_ns_id`, `metadata_region_size`); `FormatParams::new(data_disk_size, instance_id)` fills the rest from `Default`.
- `WriteHandle`: Publish/abort handle with auto-abort on drop.

#### FR-022: Supporting Types - GPU Services
- `GpuDeviceInfo`: GPU metadata (index, name, memory, compute capability, PCI bus).
- `GpuIpcHandle`: CUDA IPC memory handle with state tracking (verified, pinned).
- `GpuDmaBuffer`: GPU memory buffer with auto-close on drop.
- `GpuStream`: Opaque CUDA stream handle.

#### FR-023a: Supporting Types - Serving-Tier Attribution *(New 2026-10-01)*
- `ServedBy`: 6-variant enum naming **how** a looked-up key was served — `Dram`, `Ssd`,
  `Remote`, `Miss`, `SizeMismatch`, `Error` — with `is_hit()` true for the first three. **The
  value space is defined here and only here**; every other document and crate references this
  type rather than restating it, because a taxonomy written down twice is a taxonomy that will
  disagree with itself. It describes the **route**, not the entry's residency afterwards: an SSD
  hit that is promoted into DRAM as part of being served is `Ssd`, because that is what the
  request cost.
- `LookupOutcome`: the per-key result of `IDispatcher::batch_lookup` — `{ served_by: ServedBy,
  result: Result<(), DispatcherError> }`, with the invariant `served_by.is_hit()` **if and only
  if** `result.is_ok()`. A struct rather than `Result<ServedBy, DispatcherError>` deliberately:
  the tier-on-`Ok` encoding cannot express `Miss`, `SizeMismatch` or `Error`, since those *are*
  the `Err` cases, and would push a third of the taxonomy into a per-server error-to-tier
  mapping — which is how two servers reporting the same cache would drift apart.
- Owning feature: `components/dispatcher/specs/002-served-by-tier-attribution`.

#### FR-023: Supporting Types - Remote
- `RemoteLookupError`: 2-variant error enum (`NotFound`, `TransportError`).
- `RemoteServeStats` *(Sync 2026-10-05 — CODE CHANGE)*: **4-field** `Copy` counter snapshot returned by `IRemoteLookup::serve_stats` — `peer_served_keys`, `peer_triggered_promotions`, `peer_pins_held`, `peer_pin_hold_us_max`. `peer_triggered_promotions` is a **first-touch** quantity (promoting the key leaves it warm), so it is meaningful as a rate and misleading as a lifetime ratio against the first.
  - **`peer_pins_held` is a GAUGE and the only non-monotonic field in this type.** It rises as a serve pins keys and falls when the batch is released, so differencing two snapshots is meaningless — it is read as a level. A level that does not return to zero between serves is a leaked pin, which is otherwise undetectable: `read_ref` carries no owner identity, so a leaked pin is indistinguishable from a live reader and makes its entry permanently unevictable.
  - Hold time is attributed **per batch, not per key**: one batch pinning *n* keys for one interval blocks eviction for that interval, not *n* times it. A batch that pinned nothing records nothing, since it blocked nothing.
  - `peer_pin_hold_us_max` is the **only** hold-time field, and it is a high-water mark rather than an average by design: one batch held for seconds among thousands of fast ones stalls the tier for that whole window, which is exactly the case an average would hide.
  - ~~`peer_pins_taken`~~ / ~~`peer_pin_hold_us_total`~~ — **SUPERSEDED 2026-10-05, removed.** Both were published in the 6-field version of this type and are gone. `peer_pins_taken` measured **1.000×** `peer_served_keys` across all three measured runs — an exact duplicate of an already-published field, and the only per-key atomic among these counters. `peer_pin_hold_us_total` could not produce the mean it existed for: it accumulated per **batch** while every key count here is per **key**, so the quotient is batch-µs-per-key rather than a mean hold, and no batch count is exposed anywhere to form a real one. A counter that cannot answer its own question is removed rather than left to be misread.
- `LookupConfig`: 12-field configuration for `IRemoteLookup::initialize` — `group`, `quorum_pct`, `phase1_timeout`, `op_deadline`, `caller_wait` (optional; how long `batch_lookup` blocks the caller, `None` = until `op_deadline`), `connection_teardown_timeout` (grace before an orphaned landing slot's peer QP is torn down and the slot reclaimed; default 1000 ms), `max_retry_rounds`, `max_keys_per_query`, `bind_ip`, `actor_cpu`, `discovery` (optional `GossipConfig`, see FR-032), `node_endpoint`. Implements `Default`. Semantics of `caller_wait` and `connection_teardown_timeout` are specified in remote-lookup spec 002.
- ~~`LookupRef`~~ / ~~`RemoteRequestHandlerError`~~ — **SUPERSEDED**, removed together with `IRemoteRequestHandler` (see FR-013). No replacement type exists: the RDMA split's writes are one-sided (no zero-copy handle is returned to a caller) and its errors are `RemoteLookupRdmaInitiatorError`/`RemoteLookupRdmaResponderError` (FR-033/FR-034).

#### FR-031a: IDispatcher - Scheduling Persistence *(New 2026-10-02)*
- `schedule_write_through(key, size)`: enqueue a background write-through for an entry already resident in the memory tier, **without blocking**. Distinct from `flush_to_ssd`, which blocks until every queued job lands and is therefore unusable from a latency-sensitive caller.
- **Why the interface needs this at all.** A client store is persisted as a side effect of `copy_gpu_to_memory_completed`. A value fetched from a peer is published straight into the dispatch map by `remote-lookup`, which holds no route to the background writer, so without this method such an entry keeps `ssd_offset == None` permanently. `IDispatchMap::try_evict_to_block` refuses an unpersisted entry and the dispatcher's clean-eviction scan will not `remove` one either (removing a resident key turns it into `NotExist` under the Check→Pin race), so the entry has **no exit from the memory tier**: it ages to the oldest end of the LRU and the eviction scan, which samples exactly the oldest keys, comes to see nothing else.
- Implementations MUST NOT block. The caller is a poll loop whose stalling stops all peer messaging.
- Best-effort: a failure MUST NOT fail the caller's operation. The key is correctly cached and readable either way; only its demotability is lost, which is the pre-existing behaviour.

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
- The dispatcher SHALL maintain a bounded pool of pre-registered pinned host DRAM staging buffers for SSD→GPU cold loads that cannot obtain a memory-tier slot under pressure, sized by `DispatcherConfig::cold_staging_slots` (buffer count, default 64; 0 disables staging so cold loads fail on a full memory tier) and `DispatcherConfig::cold_staging_buf_bytes` (per-buffer byte capacity, must be ≥ the largest per-block transfer size, default 16 MiB — raised from 4 MiB in commit `8e12e49f` because a 64-token KV-offload block is 8 MiB per worker and the old 4 MiB buffer failed the `serve_cold_staged` fit guard on every tier-full cold load; see dispatcher FR-033), bounding concurrent cold-read parallelism so a burst cannot exhaust the memory tier.

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

#### FR-035: ITransferEndpoint / ITransferEndpointAdmin Interfaces
- Ungated: the implementor's receptacles are `ILogger`, `IDispatchMap` and `IMemoryTier`, all available without `spdk` (plus `IDispatcher` in an `spdk` build of the implementor). The behavioural contract — wire protocol, credits, deadlines, teardown-before-reclaim — is owned by `components/certus-transfer-endpoint/specs/001-rdma-cache-transfer` (spec and `contracts/itransfer_endpoint.md`) and is referenced, not restated, here.
- `ITransferEndpoint` (6 methods):
  - `fetch(peer: &str, items: &[(CacheKey, LocalSlot)], on_complete: FetchCompletion) -> Result<TransferContextId, TransferEndpointError>` — destination side: each entry lands in a `LocalSlot` of the endpoint's own memory-tier pool, refused (`Transport`, nothing sent) unless wholly inside it; the endpoint supplies the pool's rkey. Any length is accepted, chunked by the peer's `max_entries`; `on_complete` fires exactly once if this returns `Ok`, never if it returns `Err`. A peer with no version in common is found only at the capability exchange, so it reaches `on_complete` as `IncompatiblePeer`.
  - `landing_region() -> Result<LandingRegion, TransferEndpointError>` — the pool as peers write into it (address, length, rkey), registered once at `initialize`; what a legacy control plane hands a pusher.
  - `push_async(endpoint: &str, items: &[(CacheKey, RemoteRegion)], on_complete: FetchCompletion) -> Result<(), TransferEndpointError>` and `push(endpoint, items) -> Result<Vec<EntryOutcome>, TransferEndpointError>` — the legacy source-initiated subset, signature-compatible with `IRemoteLookupRdmaInitiator` but for the per-item outcome type.
  - `disconnect(peer: &str) -> Result<(), TransferEndpointError>` — teardown-before-reclaim barrier.
  - `stats() -> TransferStats`.
  - There is deliberately no `serve` method: the source side is reactive. Every `peer`/`endpoint` is a normalized `"host:port"`; a missing port is `InvalidEndpoint`.
- `ITransferEndpointAdmin` (3 methods): `configure(config: EndpointConfig)`, `initialize()`, `shutdown()` (the only way to stop a running endpoint; idempotent). Until `initialize` succeeds every `ITransferEndpoint` method that can fail returns `NotInitialized`.

#### FR-036: Supporting Types - Transfer Endpoint
- `LocalSlot`: `Copy` 2-field destination slot (`addr: u64`, `length: u32`) — a memory-tier buffer the caller has allocated and not published.
- `LandingRegion`: `Copy` 3-field pool registration (`addr: u64`, `len: u64`, `rkey: u32`; `len` is a `u64` since a pool can exceed 4 GiB) with bounds-checked `region_at(offset: u64, len: u32) -> Option<RemoteRegion>`; valid across reconnects.
- `EntryOutcome`: `Copy` 4-variant per-entry outcome (`HitMemoryTier`, `HitBlockTier`, `Miss`, `Failed`); `is_hit()` true for the first two. `Failed` is kept apart from `Miss` so a broken path is never read as a cold cache.
- `TransferContextId`: `Copy + Ord + Hash` newtype over `u64` naming one fetch; a correlation id, not a cancellation handle.
- `TransferEndpointError`: 6-variant error enum (`NotInitialized`, `InvalidEndpoint`, `Deadline`, `Protocol`, `IncompatiblePeer`, `Transport`), each carrying a `String`. `IncompatiblePeer` covers every failed capability exchange (no version in common, a zero quota or `max_entries`).
- `FetchResult`: alias `Result<Vec<EntryOutcome>, TransferEndpointError>`; `FetchCompletion`: `Box<dyn FnOnce(FetchResult) + Send>`, run on the endpoint thread and must not block.
- `TransferStats`: `Copy + Default` 22-field counter snapshot: 5 entry-outcome counters, 3 request/chunk counters, 4 live gauges (send-queue requests in flight and its peak; requests outstanding outbound and in service inbound, read from the connections at each snapshot), 2 bounce-buffer gauges, 4 block-read counters, and `deadline_expiries`, `protocol_faults` (every `OP_ERROR` sent or received, and every teardown an inbound message causes), `version_downgrades`, `duplicate_connections_resolved`.
- `EndpointConfig`: 10-field start-up configuration (`local`, `max_send_wr`, `max_recv_wr`, `send_credits`, `message_credits`, `inbound_quota`, `actor_cpu`, `fetch_deadline`, `deadline_action`, `block_readers`); `EndpointConfig::new(local)` defaults 128 / 64 / 8 / 4 / 4 / `None` / `Some(5 s)` / `FailLoud` / 4, the credit defaults sized by measurement (certus-transfer-endpoint research D10). `configure` refuses zero `message_credits`, `inbound_quota` or `block_readers`, and `initialize` refuses zero `send_credits`.
- `DeadlineAction`: 2-variant enum, `FailLoud` (default) and `ReportPartial` (deferred; refused at `configure`, never silently run as `FailLoud`).
- `RemoteRegion` (FR-033) is reused verbatim, not redefined.

### Non-Functional Requirements

#### NFR-001: Zero Implementation Coupling
- Components depend only on `interfaces` for trait definitions, never on sibling implementation crates.

#### NFR-002: Feature-Gated Compilation
- SPDK-dependent code compiles only with `--features spdk`, allowing the default build to work without SPDK installed.
- The `gpu` feature is declared but currently gates nothing (reserved, see Overview). `IGpuServices` and its types compile in every build; its methods that take SPDK `DmaBuffer`s are gated behind `spdk`.

#### NFR-003: Thread Safety
- All types that cross thread boundaries implement `Send` (with documented SAFETY justifications for unsafe impls).
- `DmaBuffer`, `GpuIpcHandle`, `GpuDmaBuffer`, `GpuStream`, `LookupResult` are `Send + Sync`.
- `BounceBuffer` is `Send` (hand-written, SAFETY-justified: whoever holds a buffer owns its bytes until it is freed, and the pool outlives every buffer) but deliberately **not** `Sync`.
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
- Verification status is not recorded in the interface definitions: per-property proof claims were removed from `IDispatchMap`, `IDispatcher`, `IExtentManager` and `IMemoryTier` in `a93b5620f` (2026-08-20) and live with each component's verification artifacts. `IGpuServices` still carries a comment summarising `components/gpu-services/verif/` (`src/igpu_services.rs:250-265`).
