# Certus System Architecture — `full-optimized` Profile

This document describes the architecture of the Certus storage system as a developer reference, for the `full-optimized` profile.

> **Profile summary.** `full-optimized` is the `full` profile (SPDK NVMe + GPU, standard `DispatcherComponent`) with one substitution: the shared eviction policy is **`EvictionPolicyOptimizedComponent`** — an O(1) index-linked LRU with a **TinyLFU (Count-Min Sketch) admission gate** — instead of `EvictionPolicyLruComponent`. Wiring, init order, exports, data paths and CLI are otherwise identical. See §5.2a and §8.

## 1. What is Certus?

Certus is a **generative domain-specific storage system for AI inferencing workloads**. It functions as a GPU KV-cache offloading engine: tensor data produced during inference is cached in DRAM and persisted to NVMe SSDs, then served back to GPU memory on demand via zero-copy DMA transfers.

The system sits between GPU inference engines (e.g., vLLM) and NVMe storage, providing:

- **Sub-millisecond warm lookups** — GPU←DRAM via CUDA async memcpy
- **Pipelined cold reads** — NVMe→DRAM→GPU with overlapped I/O and DMA
- **Write-through persistence** — DRAM cache with background SSD write-through
- **Pluggable eviction** — Two-tier eviction (DRAM pool and SSD capacity); this profile orders DRAM and dispatch-map candidates with LRU + TinyLFU admission

## 2. High-Level Data Flow

```
┌──────────────┐  shmq + IPC   ┌─────────────────────────────────┐
│  GPU Client  │◄─────────────►│         certus-server           │
│  (vLLM)      │ (/dev/shm)    │  (Dispatcher + component stack) │
└──────────────┘               └──────────┬──────────────────────┘
                                          │
              ┌───────────────────────────┬┴──────────────────────┐
              ▼                           ▼                        ▼
    ┌──────────────────┐     ┌───────────────────┐    ┌────────────────┐
    │  Memory-Tier     │     │  Dispatch Map     │    │  GPU Services  │
    │  (DRAM Pool)     │     │  (Index + Refs)   │    │  (CUDA DMA)    │
    └────────┬─────────┘     └───────────────────┘    └────────────────┘
             │
             ▼
    ┌──────────────────┐     ┌───────────────────┐
    │  Block Device    │────►│  Extent Manager   │
    │  (SPDK NVMe)     │     │  (Space Alloc)    │
    └──────────────────┘     └───────────────────┘
```

### Populate (PUT) Path

1. Client sends key + CUDA IPC handle via shmq (`/dev/shm` mailbox)
2. Dispatcher opens IPC handle, evicts DRAM if needed
3. `cudaMemcpy` (D2H): GPU → memory-tier slot
4. Entry registered in dispatch-map; acknowledgement sent to client
5. Background writer asynchronously persists to SSD (write-through)

### Lookup (GET) Path — Warm

1. Client sends key + destination IPC handle via shmq (`/dev/shm` mailbox)
2. Dispatch-map lookup returns MemoryTier pointer
3. `cudaMemcpyAsync` (H2D): memory-tier → GPU (via dedicated CUDA stream)
4. Stream handle returned; client synchronizes before accessing data

### Lookup (GET) Path — Cold

1. Dispatch-map lookup returns BlockDevice offset
2. Memory-tier slot allocated (inline eviction); NVMe reads DMA straight into the slot (zero-copy)
3. Batched cold keys fan out across all drives in one multiplexed scatter-gather loop; each object's H2D DMA is issued as soon as its last segment lands
4. Entry promoted back to memory-tier in place for future warm access
5. If the tier is saturated, the block is served uncached through a staging buffer (SSD → staging → GPU)

## 3. Component Framework

Certus is built on a **COM-inspired Rust component framework** that enables independent development and integration of components with low coupling.

### Core Abstractions

| Concept | Description |
|---------|-------------|
| **Interface** | A trait declared with `define_interface!`. Queryable at runtime via `IUnknown`. |
| **Component** | Declared with `define_component!`. Implements one or more interfaces. |
| **IUnknown** | Base trait for all components — provides `query_interface`, version, introspection. |
| **Receptacle** | Typed slot (`Receptacle<dyn ITrait>`) for declaring required dependencies. |
| **Binding** | Wiring components together — first-party (`receptacle.connect(arc)`) or third-party (`bind(provider, iface_name, consumer, recept_name)`). |
| **Actor** | Dedicated OS thread with lock-free channel communication. |
| **Channel** | Lock-free SPSC and MPSC implementations with backend adapters. |

### Example: Defining a Component

```rust
define_component! {
    pub DispatcherComponent {
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
        fields: {
            initialized: AtomicBool,
            bg_writer: Mutex<Option<ParallelBackgroundWriter>>,
            bg_evictor: Mutex<Option<BackgroundEvictor>>,
            bg_mt_evictor: Mutex<Option<MemoryTierEvictor>>,  // DRAM -> SSD demotion
            data_drives: RwLock<Vec<DataDrive>>,
            pipeline_ring: RwLock<Option<pipeline::PipelineRing>>, // owns the cold-load StagingPool
            warm_stream: AtomicU64,
            block_device_factory: Mutex<Option<BlockDeviceFactory>>,
            extent_manager_factory: Mutex<Option<ExtentManagerFactory>>,
            max_eviction_attempts: AtomicUsize,
            store_backpressure_ms: AtomicU64,
            pipeline_metrics: RwLock<Option<Arc<dyn PipelineMetrics>>>,
            eviction_tx: Arc<Mutex<Option<EvictionSender>>>,  // best-effort EvictionEvent channel
            eviction_dropped: Arc<AtomicU64>,                 // events the channel could not take
            tier_counters: Arc<TierEventCounters>,
        },
    }
}
```

### Component Lifecycle

1. **Instantiate**: `Component::new_default()` or `Component::new(fields...)`
2. **Wire receptacles**: `component.receptacle.connect(arc_to_provider)`
3. **Query interface**: `query_interface!(component, ITrait)`
4. **Initialize**: Call trait-specific `initialize()` method
5. **Use**: Call interface methods
6. **Shutdown**: Call `shutdown()`, then drop

### Workspace Layout

```
lib/
├── component-framework/     # Core framework (component-core, component-macros, facade)
├── shm-queue/               # /dev/shm mailbox transport
├── shmq-dispatcher/         # shmq ops -> IDispatcher translation + serve loop
└── spdk-sys/                # Raw FFI bindings to SPDK
components/
├── interfaces/              # All interface trait definitions
├── dispatcher/              # Central orchestrator (IDispatcher)
├── dispatcher-p2p/          # P2P variant with GPUDirect cold path (IDispatcher)
├── dispatch-map/            # Key→location index (IDispatchMap)
├── memory-tier/             # DRAM cache pool (IMemoryTier)
├── block-device-spdk-nvme/  # NVMe driver (IBlockDevice)
├── block-device-filesys/    # Filesystem-backed block device (IBlockDevice)
├── block-device-kernel/     # Kernel block device (IBlockDevice)
├── extent-manager/          # Disk space allocator (IExtentManager)
├── eviction-policy-lru/     # LRU eviction policy (IEvictionPolicy)
├── eviction-policy-optimized/     # LRU + TinyLFU admission (IEvictionPolicy)
├── eviction-policy-session-lists/ # Session-list eviction policy (IEvictionPolicy)
├── disk-partition-manager/  # GPT partitioning of each data drive
├── extended-metadata-store/ # Extended-metadata partition store
├── remote-lookup/           # Remote-cache orchestrator (IRemoteLookup)
├── remote-lookup-rdma-initiator/  # Outbound RDMA push / data-holder side (IRemoteLookupRdmaInitiator)
├── remote-lookup-rdma-responder/  # Passive RDMA accept side / requester (IRemoteLookupRdmaResponder[Admin])
├── zyre/                    # Gossip/beacon peer discovery (IZyre)
├── spdk-env/                # SPDK environment wrapper (ISPDKEnv)
├── gpu-services/            # CUDA operations (IGpuServices)
├── logger/                  # Logging component (ILogger)
├── example-helloworld/      # Example component
└── console-logger/          # Example logger
apps/
├── certus-server/           # shmq server exposing IDispatcher (/dev/shm mailbox)
├── certus-server-yaml/      # YAML-profile-configured server (shmq)
├── iops-benchmark/          # NVMe IOPS benchmark
├── extent-benchmark/        # Extent manager benchmark
├── gpu-bb-vs-p2p/           # GPU bounce-buffer vs P2P benchmark
├── gpu-handle-test-server/  # GPU IPC handle test server
├── gpu-show/                # GPU device info tool
├── nvme-ns-manager/         # NVMe namespace management CLI
├── workload-generator/      # Multi-node workload generator (workload-* crates)
└── helloworld-mainline/     # Framework example app
certus-shmq-connector/       # Python vLLM offloading connector (shmq client)
```

## 4. Interface Definitions

All component interfaces are defined in `components/interfaces/src/`. SPDK-dependent interfaces are gated behind `features = ["spdk"]`.

### IDispatcher

The top-level orchestrator interface. Coordinates all cache operations.

| Method | Description |
|--------|-------------|
| `initialize(config)` | Creates N block devices + N extent managers from PCI addresses |
| `shutdown()` | Drains background writes, shuts down all drives |
| `populate(key, ipc_handle)` | GPU→DRAM DMA, registers entry, enqueues write-through |
| `batch_populate(entries)` | Populate multiple `(key, IpcHandle)` pairs in one call |
| `reserve_memory(key, size, session_id, deadline)` | Reserve a DRAM slot without DMA (returns raw pointer). Backpressures on a full tier until `deadline` (`None` = this call's own `store_backpressure_ms` budget). `session_id` is an opaque per-request id (0 = unset) for observability only |
| `copy_gpu_to_memory_async(key, regions, stream)` | Async D2H copy of GPU region(s) into a previously reserved slot on the given CUDA stream |
| `copy_gpu_to_memory_completed(key, size)` | Finalize a reserved slot after the async copy: register in dispatch-map + enqueue write-through |
| `release_memory(key)` | Cancel a reserved slot without populating |
| `lookup(key, ipc_handle)` | Serves from DRAM (warm) or promotes from SSD (cold) |
| `lookup_async(key, ipc_handle)` | Non-blocking lookup, returns CUDA stream |
| `batch_lookup(entries)` | Batch lookup over `&[(CacheKey, Vec<IpcHandle>)]` with parallel SSD promotion; returns one `LookupOutcome { served_by, result }` per entry |
| `check(key)` | Existence check without data transfer |
| `remove(key)` | Removes entry from all tiers |
| `touch(key)` | Refreshes eviction timestamp |
| `pin(key) / unpin(key)` | Pin an entry in DRAM (exempt from eviction/demotion) and release the pin |
| `promote_to_memory_tier(keys)` | Pre-promote cold entries to DRAM for future warm access |
| `clear_memory_tier()` | Evicts all DRAM entries |
| `flush_to_ssd()` | Force all pending write-through jobs to complete |
| `schedule_write_through(key, size)` | Non-blocking: enqueue write-through for a key that entered DRAM by a non-store path (e.g. a remote fetch) so it becomes demotable |
| `read_write_stats()` | Aggregate read/write byte and op counters |
| `tier_event_stats()` | Tier-transition counters (promotions, demotions, evictions), per-route hit counts, store backpressure/drop counts, and eviction-blocked diagnostics |

**Lookup attribution.** `ServedBy` (`Dram`, `Ssd`, `Remote`, `Miss`, `SizeMismatch`, `Error`) names the **route** a key was served by, not its residency afterwards: a cold hit that is promoted into DRAM while being served is still `Ssd`. `is_hit()` holds exactly when `result.is_ok()`. The shmq Lookup response carries it as a per-key byte (0 = not served, 1 = DRAM, 2 = SSD, 3 = remote).

**`DispatcherConfig`** additions relevant to this profile: `store_backpressure_ms` (default 5000), `cold_staging_slots` (default 64), `cold_staging_buf_bytes` (default 16 MiB), `metadata_partition_size` / `extended_metadata_partition_size` (128 MiB each), and the `memory_tier_eviction_*` knobs (threshold default 0.0 = MemoryTierEvictor disabled).

### IDispatchMap

The key→location index with reader/writer reference counting.

| Method | Description |
|--------|-------------|
| `initialize()` | Initializes internal state |
| `create_memory_tier_entry(key, ptr, size)` | Registers DRAM-resident entry |
| `lookup(key)` | Returns `LookupResult` enum (NotExist/MismatchSize/MemoryTier/BlockDevice) |
| `convert_to_storage(key, offset)` | Records the SSD `ssd_offset`; the entry **stays MemoryTier** (now durable/evictable) |
| `convert_memory_tier_to_block(key)` | Demotes DRAM entry to SSD-only |
| `promote_block_to_memory_tier(key, ptr, size)` | In-place BlockDevice→MemoryTier (retains `ssd_offset`); works on pinned entries |
| `try_evict_to_block(key)` | MemoryTier→BlockDevice, only if `ssd_offset` is set and the entry is unreferenced. Returns `ActiveReferences` for a held pin, `InvalidState` otherwise (counted separately) |
| `take_read/release_read` | Reference counting for concurrent access |
| `take_write/release_write` | Exclusive writer semantics |
| `downgrade_reference(key)` | Atomically downgrade write ref to read ref |
| `remove(key)` | Removes entry (fails if active references) |
| `touch(key)` | Refreshes eviction ordering |
| `entry_size(key)` | Returns the size of a stored entry |
| `oldest_keys(n)` | Returns N oldest keys for eviction |
| `is_evictable(key)` | Checks if safe to evict (write-through complete, no refs) |
| `recover_extent(key, offset, size)` | Rebuilds from persisted extents on startup |
| `set_checksum / get_checksum` | CRC-32 attached to an entry (used with the `integrity-check` feature) |

### IMemoryTier

DRAM cache pool with pluggable eviction (via `IEvictionPolicy`).

| Method | Description |
|--------|-------------|
| `initialize(pool_size, numa_node)` | Allocates mmap'd pool, optionally NUMA-pinned |
| `insert(key, size)` | Allocates slot, returns pointer |
| `get(key)` | Returns (pointer, size), refreshes eviction order |
| `peek(key)` | Returns (pointer, size) without eviction-order update |
| `evict_next()` | Removes the eviction policy's next victim |
| `evict_next_for_key(key)` | Evicts the policy's next victim from the target key's shard |
| `oldest_keys(n)` | Returns N oldest keys for eviction decisions |
| `remove(key)` | Frees specific slot |
| `touch(key) / batch_touch(keys)` | Refresh eviction-order position(s) |
| `contains(key)` | Existence check |
| `capacity() / used()` | Pool utilization metrics |
| `pool_info()` | Returns base pointer + size for CUDA registration |
| `is_dma_capable()` | Whether pool is registered for zero-copy DMA |
| `telemetry_snapshot()` | Point-in-time counters (inserts, hits, evictions, bytes resident) |
| `clear()` | Remove all entries, return count evicted |

### IBlockDevice

NVMe block device with actor-model architecture.

| Method | Description |
|--------|-------------|
| `connect_client()` | Returns (command_tx, completion_rx) channel pair |
| `block_size()` | Device logical block size |
| `sector_size(ns_id)` | Sector size for a namespace |
| `num_sectors(ns_id)` | Namespace capacity |
| `max_transfer_size()` | MDTS limit |
| `max_queue_depth()` | Max outstanding ops per queue |
| `num_io_queues()` | Number of I/O queues |
| `numa_node()` | Controller NUMA node |
| `nvme_version()` | Reported NVMe spec version |
| `telemetry()` | Controller/queue telemetry snapshot |
| `read_write_stats()` | Aggregate read/write byte and op counters |

Commands are sent as enum variants: `ReadSync`, `WriteSync`, `ReadAsync`, `WriteAsync`, `WriteZeros`, `BatchSubmit`, `AbortOp`, `ControllerReset`, `FlushSync`, and the `Ns*` namespace-management ops.

### IExtentManager

Persistent space allocator with crash-consistent metadata.

| Method | Description |
|--------|-------------|
| `format(params)` | Writes superblock and initializes regions |
| `initialize()` | Recovers state from existing superblock |
| `reserve_extent(key, size)` | Returns WriteHandle (publish or auto-abort on drop) |
| `remove_extent(offset)` | Frees extent space |
| `checkpoint()` | Persists allocation state to disk |
| `get_extents()` | Lists all committed extents |
| `used_bytes() / capacity_bytes()` | Utilization metrics |

### IGpuServices

CUDA GPU operations for DMA transfers.

| Method | Description |
|--------|-------------|
| `initialize()` | Loads CUDA runtime, discovers GPUs (compute 7.0+) |
| `dma_copy_to_host(gpu_ptr, dma_buf, size)` | GPU→DRAM synchronous copy |
| `dma_copy_to_device(dma_buf, gpu_ptr, size)` | DRAM→GPU synchronous copy |
| `memcpy_h2d_async(src, dst, size, stream)` | Async DRAM→GPU on stream |
| `create_stream() / destroy_stream()` | CUDA stream management |
| `stream_synchronize(stream)` | Block until stream completes |
| `allocate_pinned_dma_buffer(size)` | CUDA-pinned + SPDK-registered buffer |
| `register_host_memory(ptr, size)` | Pin existing memory for zero-copy |

### ISPDKEnv

SPDK environment initialization and lifecycle.

### ILogger

Simple logging interface (`error`, `warn`, `info`, `debug`).

## 5. Key Components in Detail

### 5.1 Dispatcher (`components/dispatcher/` and `components/dispatcher-p2p/`)

The central orchestrator implementing `IDispatcher`. Two variants exist:

- **DispatcherComponent** (`components/dispatcher/`) — Standard dispatcher
- **DispatcherP2pComponent** (`components/dispatcher-p2p/`) — Adds GPUDirect P2P ring and DRAM backfill worker

The `full-optimized` profile uses **DispatcherComponent** (same as `full`), which owns:

- **N data drives**: Each is a (BlockDevice, ExtentManager) pair created from PCI addresses
- **ParallelBackgroundWriter**: Multi-threaded write-through (drains jobs from a channel)
- **BackgroundEvictor**: Monitors SSD utilization and reclaims space
- **MemoryTierEvictor**: Background thread that demotes cold DRAM entries to SSD (DRAM→SSD), evicting only entries with `ssd_offset` set and no active references
- **ChannelPool (per drive)**: Pre-connected NVMe client channels; a cold batch checks out one lease per active drive
- **Scatter-gather cold path**: `batch_lookup` runs one multiplexed `scatter_gather_multi_drive_zero_copy` poll loop on the caller thread, fanning NVMe reads across every drive (queue depth 128, split across drives by segment count) — no per-drive worker threads
- **PipelineRing**: Holds the chunk size (drive MDTS) and the cold-load **StagingPool** — `cold_staging_slots` CUDA-pinned, SPDK-registered buffers of `cold_staging_buf_bytes` each, used to serve a cold block uncached when the memory tier is saturated
- **DeviceStreams (per GPU device)**: warm (H2D), store (D2H) and pipe[2] (cold) CUDA streams, created lazily per device so tensor-parallel GPUs each get device-correct streams
- **Warm stream**: Dedicated CUDA stream for async memory-tier→GPU copies (lock-free access via AtomicU64)
- **BlockDeviceFactory / ExtentManagerFactory**: Factory closures for creating DataDrive components during initialization
- **DiskPartitionManager (per drive)**: Lays out a GPT with three partitions — metadata, extended metadata (reserved), data — and points the drive's ExtentManager at the metadata and data base LBAs
- **EvictionSender**: Bounded, non-blocking `EvictionEvent { key, reason: Demoted | Removed }` channel drained by the shmq `TakeEvents` op; an event that cannot be queued is counted in `eviction_dropped`, never lost silently

The P2P variant additionally owns:
- **P2pRing**: GPUDirect Storage ring for direct SSD→GPU transfers
- **DramBackfillWorker**: Background thread that asynchronously promotes hot P2P entries back to DRAM

Receptacles: `dispatch_map`, `memory_tier`, `gpu_services`, `spdk_env`, `logger`, `remote_lookup`.

Key design decisions:
- Drive selection is a splitmix64 hash of the key mod `num_drives` (deterministic sharding; the write path and the SSD evictor use the same function)
- Store backpressure: `reserve_memory` retries eviction every 20 ms for up to `store_backpressure_ms` instead of failing on a momentarily full tier; if the budget elapses, `populate` drops the store best-effort (reports success, counted in `store_drops_on_full`) so a full cache is never fatal to the vLLM connector
- Memory-tier pool is registered with both CUDA (`cudaHostRegister`) and SPDK (`spdk_mem_register`) for zero-copy transfers in both directions
- Write-through uses a read reference (not write) so lookups can proceed concurrently

### 5.2 Memory-Tier (`components/memory-tier/`)

DRAM cache pool using:
- **mmap'd contiguous allocation** for the pool
- **First-fit free-list allocator** with 4 KiB alignment (`allocator.rs`)
- **Delegated eviction ordering** via a bound `IEvictionPolicy` component (`eviction-policy-optimized` in this profile)
- **HashMap<CacheKey, Slot>** for O(1) key lookup
- **Refusal-safe insert**: free space is checked before the eviction policy is asked to track the key, so a policy refusal leaves the pool unchanged and surfaces as an error rather than a panic

Component default pool size: 256 MiB; `certus-server-yaml` defaults `--memory-tier-size` to 2 GiB. The pool is CUDA-pinned and SPDK-registered at server startup for zero-copy DMA in both directions.

### 5.2a Eviction Policy (`components/eviction-policy-optimized/`) — profile-specific

`EvictionPolicyOptimizedComponent` is the one component that distinguishes `full-optimized` from `full`. It implements the same `IEvictionPolicy` interface as `eviction-policy-lru`, so memory-tier and dispatch-map are unchanged; one instance is shared and each consumer creates its own pool.

Each pool holds:
- **O(1) index-linked LRU list** (`lru_list.rs`) — nodes in a `Vec`, referenced by index, recycled through a free list. Front = least recently used (next victim), back = most recently used.
- **Count-Min Sketch** — 4 rows × 1024 saturating `u8` counters, one multiplicative hash per row. It estimates how often a key has been **tracked** (inserted); `touch` does not increment it.
- **Aging** — every `10 × max_len` tracks the whole sketch is halved, so old popularity decays.

**TinyLFU admission gate (in `track`).** When a key is inserted, its sketch count is incremented and compared with the count of the current LRU head (the next victim):

| Condition | Placement | Effect |
|-----------|-----------|--------|
| `estimate(new) <= estimate(head)` | LRU **front** | New key is not more popular than the victim — it becomes the next eviction candidate itself |
| `estimate(new) > estimate(head)` | LRU **back** (MRU) | New key is admitted ahead of the incumbent |
| pool empty | back | — |

So a one-off key that is stored once and never seen again cannot push out a key that has been re-stored repeatedly: it is queued for eviction first. `touch` / `batch_touch` move a key to the MRU end exactly as in plain LRU; `identify_next_to_evict` pops the front; `get_eviction_candidates(n)` peeks the first n from the front.

**Interaction with the dispatcher's eviction rules.** The policy only orders candidates. Eligibility is still decided by the dispatcher and dispatch-map (write-through landed, no pins), so a key placed at the front by the admission gate but not yet written through is skipped by `evict_one_clean` like any other unpersisted entry, and the scan widens past it.

Invalid pool ids are rejected (`EvictionPolicyError::InvalidPool`) and logged; handles are range-checked before the slot is read.

### 5.3 Dispatch Map (`components/dispatch-map/`)

The index tracking every cached entry's location and state. Receptacles: `eviction_policy` (IEvictionPolicy), `extent_manager` (IExtentManager), `logger` (ILogger).

```rust
enum LookupResult {
    NotExist,
    MismatchSize,
    BlockDevice { offset: u64 },
    MemoryTier { pointer: *mut u8, size: u32 },
}
```

Provides reader/writer reference counting with blocking semantics (writers block readers and vice versa, with a 2 s timeout). On startup the dispatcher iterates each data drive's extent manager and calls `recover_extent` to rebuild the map. If no eviction policy is connected, operations that need one return `NotInitialized` rather than panicking.

### 5.4 Block Device (`components/block-device-spdk-nvme/`)

High-performance NVMe driver using SPDK userspace I/O:

- **Actor model**: Dedicated thread per NVMe controller, NUMA-pinned
- **Channel-based I/O**: Clients get SPSC channel pairs (command_tx, completion_rx)
- **Zero-copy**: DMA buffers from SPDK hugepages
- **MDTS-aware**: I/O segmenter splits transfers exceeding the device's maximum data transfer size
- **Multiple queue depths**: Exploits different NVMe I/O queues for varying batch sizes

### 5.5 Extent Manager (`components/extent-manager/`)

Persistent space allocator with crash consistency:

**On-disk layout:**
```
┌─────────────┬──────────────────────┬──────────────────────┬─────────────┐
│ Superblock  │ Checkpoint Region A  │ Checkpoint Region B  │  Data Area  │
│ (4 KiB)     │ (per-region bitmaps) │ (per-region bitmaps) │             │
└─────────────┴──────────────────────┴──────────────────────┴─────────────┘
```

- **Superblock**: Magic (`CERTUSV4`), format version, geometry parameters
- **Double-buffered checkpoints**: Two copies for atomic updates (active_copy toggles)
- **Buddy allocator**: Power-of-two extent sizes within each region
- **Regions**: Configurable count (power of two), each independently managed
- **WriteHandle**: RAII pattern — `publish()` commits, `Drop` auto-aborts
- **Background checkpoint thread**: Periodic persistence (default: 30 seconds)

### 5.6 GPU Services (`components/gpu-services/`)

CUDA integration layer providing:
- Device discovery and initialization (requires compute capability 7.0+)
- IPC memory handle management (open/close cross-process GPU memory)
- Synchronous and asynchronous DMA transfers (H2D, D2H)
- CUDA-pinned host memory allocation and registration
- SPDK memory registration for NVMe DMA compatibility

## 6. Server and Client Architecture

### certus-server (`apps/certus-server/`)

A shared-memory-queue (shmq) server exposing the `IDispatcher` interface over a
`/dev/shm` mailbox file. There is no TCP port and no network transport; clients
reach the server by sharing the host IPC namespace and `/dev/shm` (podman
`--ipc=host` or k8s `hostIPC: true`).

**CLI options (`certus-server-yaml`):**
- `--device-pci` — NVMe PCI addresses (repeatable); or `--drive-count N` to use the first N discovered drives
- `--shm-path` — Shared-memory mailbox file path (default: `/dev/shm/certus-shmq`)
- `--channels` — Number of mailbox channels (= max in-flight requests = worker threads; default 8)
- `--cap-req` / `--cap-resp` — Per-channel request/response capacity (default 1M / 128K)
- `--reserve-timeout-secs` — Reclaim Reserve-without-Commit leaks after this long (default 30)
- `--shmq-poller-cpu`, `--shmq-poller-stats` — Pin the shmq poller; log its fairness/backlog stats
- `--memory-tier-size` — Pool size (e.g., `256M`, `1G`, `4G`; default 2G)
- `--format` — Format SSD extents on startup (start fresh)
- `--poller-base-cpu` — Pin NVMe poller threads from this core (otherwise NUMA-local auto-pinning)
- `--max-eviction-attempts` (default 2048), `--store-backpressure-ms` (default 5000)
- `--memory-tier-eviction-threshold` — Enable the background MemoryTierEvictor (default 0.0 = off)
- `--metrics-port`, `--otel-endpoint`, `--otel-service-name` — Prometheus / OTLP export
- `--backfill-delay-ms`, `--rl-group` — accepted but inert in `full-optimized` (p2p-only / remote-lookup not initialized)

**Startup sequence:**
1. SPDK environment initialization
2. GPU services initialization
3. Dispatch map creation
4. Memory-tier allocation + CUDA host registration
5. Dispatcher initialization (creates block devices + extent managers)
6. Thread CPU bindings logged (`log_cpu_bindings`, read back from `/proc/self/task`)
7. shmq mailbox creation + poller/worker/reaper start

**shmq ops** (opcode-framed binary wire: `lib/shmq-dispatcher/src/wire.rs`):
- `Populate` — Batch insert from GPU memory
- `Lookup` — Batch retrieve to GPU memory
- `Check` — Batch existence check
- `Remove` — Batch delete
- `Touch` — Batch LRU refresh
- `TouchCheck` — Touch + Check fused in one server pass
- `ClearMemoryTier` — Evict all DRAM entries
- `FlushToSsd` / `GetIoStats` — flush write-through / read I/O counters

The full `IDispatcher` surface (Reserve, CopyToStore, CommitStore, AbortStore,
Pin, Unpin, TakeEvents, …) is carried as additional opcodes. All operations
accept batches and use CUDA IPC handles (64-byte `cudaIpcMemHandle_t`) for
cross-process GPU memory sharing (the shared IPC namespace lets the server open
the client's handles). Opened IPC mappings are cached and closed after a grace
period; the `shmq-reaper` thread closes expired mappings and stale reservations
on a timer, so an idle or vanished client does not pin them for the server's
lifetime.

### certus-shmq-connector (`certus-shmq-connector/`)

A pure-Python vLLM KV-offloading connector that talks to certus-server over the
shmq mailbox (`ring.py` mirrors `wire.rs` byte-for-byte). Stores use
Reserve → CopyToStore → CommitStore (AbortStore on failure); loads use Lookup
with one IPC handle per layer tensor.

## 7. Concurrency Model

### Reference Counting (Dispatch Map)

The dispatch-map uses reader/writer references to coordinate concurrent access:

- **Populate**: Acquires write ref → downgrades to read ref for background writer
- **Lookup**: Acquires read ref → released after DMA copy
- **Remove**: Blocks if write ref active; fails if read refs active
- **Eviction**: Skips entries with active references

### Background Threads

| Thread | Purpose |
|--------|---------|
| `dispatcher-bg-writer` | Parallel write-through: memory-tier → SSD |
| `dispatcher-bg-evictor` | Monitors SSD utilization, reclaims extents |
| `dispatcher-bg-mt-evictor` | Demotes cold DRAM entries to SSD (DRAM→SSD) when the memory tier is under pressure (only started if `--memory-tier-eviction-threshold` > 0) |
| `shmq-reaper` | Reclaims stale reservations and expired CUDA-IPC mappings |
| shmq poller + workers | One poller over the mailbox channels, one worker per channel |
| `extent-mgr-checkpoint` | Periodic checkpoint of allocation metadata |
| Block device actor threads | One per NVMe controller, NUMA-pinned |

### Cold-Path Pipeline (`components/dispatcher/src/pipeline.rs`)

Cold reads are zero-copy: the memory-tier pool is SPDK- and CUDA-registered, so NVMe DMAs straight into the promoted slot and the GPU DMAs straight out of it.

- **`batch_lookup`** — `scatter_gather_multi_drive_zero_copy`: one `DriveWork` per active drive (a checked-out channel + its `ColdReadJob`s); a single sliding-window poll loop on the caller thread keeps NVMe queues full across all drives and issues each object's fused H2D on the per-device pipe streams as soon as its last segment completes. One final dual stream sync covers the batch.
- **`lookup` / `lookup_async`** — `promote_and_serve` → `pipelined_multi_object_zero_copy` for the single key.
- **Staging fallback** — `serve_cold_staged` checks out one StagingPool buffer and serves SSD → staging → GPU without caching.
- **Warm overlap** — in a mixed batch the warm H2D copies are queued first and their stream sync is deferred until after the cold scatter-gather, so the warm drain overlaps the NVMe reads. Warm read pins are held until that sync.

## 8. Eviction Policies

### DRAM Eviction

- **Trigger**: Memory-tier slot allocation fails (populate / reserve / cold promotion)
- **Policy**: LRU + TinyLFU admission via `eviction-policy-optimized` — victims come from the LRU front, where newly stored keys that are no more popular than the current victim are also placed; the scan widens past ineligible candidates
- **Eligibility**: Write-through must be complete (ssd_offset set) and no active references
- **Outcome**: Dispatch-map entry transitions MemoryTier → BlockDevice
- **No blind eviction**: unpersisted or pinned entries are skipped, never removed — removing a just-Checked key would turn a later load into a fatal miss. If nothing is evictable, the store path backpressures (then drops the store) and the load path falls back to the staging buffer
- **Diagnostics**: `evictions_blocked_by_pin`, `evictions_blocked_unpersisted`, `eviction_scans_exhausted`, and the `oldest_sampled` / `oldest_persisted` gauges in `tier_event_stats()`

### SSD Eviction (Background Evictor)

- **Trigger**: SSD utilization exceeds threshold (default: 90%)
- **Target**: Low-watermark (default: 80%)
- **Policy**: Oldest keys from dispatch-map, ordered by the same optimized policy (dispatch-map has its own pool) (removed from the dispatch-map first, which refuses pinned entries, then the DRAM slot is freed)
- **Batch size**: Configurable (default: 64 extents per sweep)
- **Interval**: Configurable (default: 5 seconds between checks)
- **Outcome**: Extent freed, entry removed from dispatch-map entirely

## 9. Crash Recovery

Certus operates under **cache semantics** — data loss on crash is acceptable because the source of truth lives elsewhere (e.g., GPU recomputation).

On restart:
1. Memory-tier (DRAM) is empty — all volatile data is lost
2. Dispatch-map is rebuilt by iterating finalized extents from each extent manager
3. Non-finalized extents (incomplete writes) are reclaimed as free space
4. System resumes with only SSD-persisted entries visible

## 10. Build System

### Default Build (no SPDK)

```bash
cargo build          # Builds: component-framework, example-helloworld, logger, gpu-services
cargo test --all     # Tests default members only
```

### Full Build (requires SPDK)

```bash
cargo build --workspace    # All members including SPDK-dependent crates
cargo build -p certus-server
```

### Feature Gates

| Feature | Crate | Effect |
|---------|-------|--------|
| `spdk` | interfaces | Enables SPDK-dependent interface traits and types |
| `gpu` | gpu-services | Enables real CUDA FFI (vs. stub) |
| `telemetry` | block-device-spdk-nvme | Enables I/O latency/throughput collection |
| `hardware-test` | dispatcher | Enables integration tests requiring real NVMe |
| `integrity-check` | dispatcher | CRC-32 each block on store, verify before the load H2D |
| `pipeline-telemetry` | dispatcher | Per-batch cold-pipeline timing logs |
| `rw-telemetry` | dispatcher, certus-server-yaml | Enables `read_write_stats()` counters |
| `otel` | certus-server-yaml | OTLP metrics export |
| `testing` | extent-manager | Exposes test utilities and superblock internals |

### SPDK Dependencies

SPDK must be pre-built at `deps/spdk-build/`. Requires:
- Kernel boot params: IOMMU + 1G hugepages
- `memlock` set to unlimited
- NVMe devices bound to `vfio-pci`

## 11. Testing Strategy

- **Unit tests**: Mocked dependencies (MockDispatchMap, MockMemoryTier, MockGpuServices)
- **Integration tests**: Full component wiring without hardware
- **Hardware tests**: Feature-gated (`--features hardware-test`), require real NVMe
- **Benchmarks**: Criterion-based for channels, NUMA, I/O, dispatch-map, dispatcher
- **CI**: GitHub Actions on `ubuntu-latest`, default members only, single-threaded

## 12. Key Design Decisions

1. **Cache, not storage**: No durability guarantees. Crash loses in-flight data. Source of truth is external.

2. **No P2P GPU↔SSD DMA**: All transfers bounce through DRAM (memory-tier). This enables promotion-on-read (cold→warm) and simplifies GPU memory management.

3. **Write-through, not write-back**: Data persists to SSD asynchronously after acknowledgement. Reads can be served from DRAM immediately.

4. **Component isolation**: Each component has its own CLAUDE.md, tests, and benchmarks. LLM context stays small by scoping to one component + its interface bindings.

5. **Deterministic sharding**: a splitmix64 hash of the key mod `num_drives` selects the target SSD. Simple, no coordination needed for single-process deployment.

6. **Zero-copy pipeline**: Memory-tier pool is simultaneously CUDA-pinned and SPDK-registered, enabling direct NVMe reads into GPU-accessible memory without intermediate copies.

7. **A full cache is never fatal**: stores backpressure then drop; cold loads fall back to staging. Both exist because a failed transfer kills the vLLM engine (`assert transfer_result.success`).

8. **Remote lookup is wired but inert in `full-optimized`**: `RemoteLookupComponent` is bound to the dispatcher but has no `init_hook`, so it never starts its actor and every forwarded local miss returns `NotFound` immediately. The `full-remote` profile initializes it.

9. **Eviction policy is a build-time substitution**: `full-optimized` differs from `full` only in the `eviction_policy` component's crate and factory. Because both implement `IEvictionPolicy`, no other component changes; the policy choice is fixed when `certus-server-yaml` is built with `CERTUS_PROFILE=full-optimized`.
