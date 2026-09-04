# Certus System Architecture (`full-p2p` Profile)

This document describes the architecture of the Certus storage system as a developer reference, for the **`full-p2p`** profile.

The `full-p2p` profile is identical to the `full` profile in component composition, wiring, and initialization order **except for one substitution**: the dispatcher is the `dispatcher-p2p` crate (`DispatcherP2pComponent`) instead of the standard `dispatcher` crate. This swaps the cold read path from a DRAM-bounce pipeline to a **GPUDirect Storage** path that DMAs data from NVMe into a GPU BAR1 staging ring and then device-to-device into the client's VRAM — a single PCIe hop, no DRAM bounce. Everything else (put path, warm path, eviction, miss-forwarding) is the same as `full`.

## 1. What is Certus?

Certus is a **generative domain-specific storage system for AI inferencing workloads**. It functions as a GPU KV-cache offloading engine: tensor data produced during inference is cached in DRAM and persisted to NVMe SSDs, then served back to GPU memory on demand via zero-copy DMA transfers.

The system sits between GPU inference engines (e.g., vLLM) and NVMe storage, providing:

- **Sub-millisecond warm lookups** — GPU←DRAM via CUDA async memcpy
- **GPUDirect cold reads** — NVMe→GPU BAR1 staging ring→GPU VRAM, bypassing the DRAM bounce buffer (this profile's distinguishing feature)
- **Write-through persistence** — DRAM cache with background SSD write-through
- **Pluggable eviction** — Two-tier eviction (DRAM pool and SSD capacity), plus background DRAM→SSD demotion

## 2. High-Level Data Flow

```
┌──────────────┐  shmq + IPC   ┌─────────────────────────────────┐
│  GPU Client  │◄─────────────►│         certus-server           │
│  (vLLM)      │ (/dev/shm)    │  (DispatcherP2p + component stk)│
└──────────────┘               └──────────┬──────────────────────┘
                                          │
              ┌───────────────────────────┬┴──────────────────────┐
              ▼                           ▼                        ▼
    ┌──────────────────┐     ┌───────────────────┐    ┌────────────────┐
    │  Memory-Tier     │     │  Dispatch Map     │    │  GPU Services  │
    │  (DRAM Pool)     │     │  (Index + Refs)   │    │  (CUDA DMA)    │
    └────────┬─────────┘     └───────────────────┘    └────────────────┘
             │                                                 ▲
             ▼                                                 │ P2P (BAR1 ring)
    ┌──────────────────┐     ┌───────────────────┐             │  device-to-device
    │  Block Device    │────►│  Extent Manager   │─────────────┘
    │  (SPDK NVMe)     │     │  (Space Alloc)    │
    └──────────────────┘     └───────────────────┘
```

### Populate (PUT) Path

Identical to the `full` profile.

1. Client sends key + CUDA IPC handle via shmq (`/dev/shm` mailbox)
2. Dispatcher reserves a DRAM slot, evicting if needed (`reserve_memory`)
3. `copy_gpu_to_memory_async` — async D2H (GPU → memory-tier slot) on the per-drive `store` stream
4. `copy_gpu_to_memory_completed` — entry registered in dispatch-map as `MemoryTier`; write-through WriteJob enqueued; acknowledgement sent to client
5. Background writer asynchronously persists to SSD (write-through), then records `ssd_offset`

### Lookup (GET) Path — Warm

Identical to the `full` profile.

1. Client sends key + destination IPC handle via shmq (`/dev/shm` mailbox)
2. Dispatch-map lookup returns MemoryTier pointer
3. `cudaMemcpyAsync` (H2D): memory-tier → GPU (via dedicated `warm_stream`)
4. Stream handle returned; client synchronizes before accessing data

### Lookup (GET) Path — Cold (P2P BAR1, **profile-specific**)

1. Dispatch-map lookup returns a `BlockDevice { offset }` result
2. The `P2pColdReadPool` schedules the read on a per-drive worker
3. NVMe reads DMA the block into the **GPU BAR1 staging ring** — GPU device memory exposed on the PCIe BAR, mapped via GDRCopy and registered with SPDK so the NVMe controller can DMA into it directly (`P2P_RING_SLOTS = 64` slots)
4. A **device-to-device** copy moves each slot's contents from the BAR1 ring into the client's destination VRAM
5. **The dispatch-map entry stays `BlockDevice`** — the P2P serve does not promote to DRAM
6. A `DramBackfillJob` is queued so the `DramBackfillWorker` can asynchronously read the block SSD→DRAM later and flip the entry to `MemoryTier` (making subsequent lookups warm)

This is a **single PCIe hop** (NVMe→GPU) rather than the two hops of the DRAM-bounce cold path (NVMe→DRAM, then DRAM→GPU).

> **Fallback:** the P2P path serves **single-region** blocks only. Multi-region lookups (a key mapped across several GPU destination regions) are rejected with `InvalidParameter`. If the P2P ring is unavailable (not initialized), the dispatcher falls back to a DRAM-bounce promote-and-serve like the base cold path.

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

### Example: The P2P Dispatcher Component

The `full-p2p` profile wires `DispatcherP2pComponent`. It declares the same
receptacles as the standard dispatcher but carries additional fields for the P2P
cold path:

```rust
define_component! {
    pub DispatcherP2pComponent {
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
            bg_backfill: Mutex<Option<DramBackfillWorker>>,   // P2P: async NVMe -> DRAM backfill
            cold_pool: Mutex<Option<P2pColdReadPool>>,        // P2P cold read worker pool
            p2p_ring: RwLock<Option<P2pRing>>,                // GPU BAR1 staging ring
            data_drives: RwLock<Vec<DataDrive>>,
            pipeline_ring: RwLock<Option<PipelineRing>>,      // DRAM-bounce fallback
            warm_stream: AtomicU64,
            block_device_factory: Mutex<Option<BlockDeviceFactory>>,
            extent_manager_factory: Mutex<Option<ExtentManagerFactory>>,
            // ...
        },
    }
}
```

The `dispatcher-p2p` crate is organized into modules: `lib.rs` (component +
`IDispatcher` impl), `p2p_ring.rs` (BAR1 staging ring), `cold_pool.rs`
(`P2pColdReadPool` worker pool + DRAM fallback), `background.rs`
(`DramBackfillWorker`, `MemoryTierEvictor`), `pipeline.rs` (DRAM-bounce pipeline),
`io_segmenter.rs` (MDTS-aware chunking), and `pins.rs` (dispatch-map read-pin
helpers).

### Component Lifecycle

1. **Instantiate**: `Component::new_default()` or `Component::new(fields...)`
2. **Wire receptacles**: `component.receptacle.connect(arc_to_provider)`
3. **Query interface**: `query_interface!(component, ITrait)`
4. **Initialize**: Call trait-specific `initialize()` method
5. **Use**: Call interface methods
6. **Shutdown**: Call `shutdown()`, then drop

### Workspace Layout

```
components/
├── component-framework/     # Core framework (3 crates)
│   └── crates/
│       ├── component-core/  # Traits, actor, channels, NUMA
│       ├── component-macros/# Proc macros (define_interface!, define_component!)
│       └── component-framework/ # Facade re-export
├── interfaces/              # All interface trait definitions
├── dispatcher/              # Standard orchestrator (IDispatcher)  — NOT used by full-p2p
├── dispatcher-p2p/          # P2P variant with GPUDirect cold path (IDispatcher)  — used by full-p2p
├── dispatch-map/            # Key→location index (IDispatchMap)
├── memory-tier/             # DRAM cache pool (IMemoryTier)
├── block-device-spdk-nvme/  # NVMe driver (IBlockDevice)
├── extent-manager/          # Disk space allocator (IExtentManager)
├── eviction-policy-lru/     # LRU eviction policy (IEvictionPolicy)
├── remote-lookup/           # Remote-cache orchestrator placeholder (IRemoteLookup)
├── spdk-env/                # SPDK environment wrapper (ISPDKEnv)
├── spdk-sys/                # Raw FFI bindings to SPDK
├── gpu-services/            # CUDA operations, incl. P2P/BAR1 mapping (IGpuServices)
├── logger/                  # Logging component (ILogger)
└── console-logger/          # Example logger
apps/
├── certus-server/           # shmq server exposing IDispatcher (/dev/shm mailbox)
├── certus-server-yaml/      # YAML-profile-configured server (shmq)  — hosts full-p2p
├── gpu-bb-vs-p2p/           # GPU bounce-buffer vs P2P benchmark
└── ...
certus-connector/            # PyO3 module for vLLM integration
```

## 4. Interface Definitions

All component interfaces are defined in `components/interfaces/src/`. SPDK-dependent interfaces are gated behind `features = ["spdk"]`. The `full-p2p` profile provides exactly the same interfaces as `full`; the P2P behavior lives inside the dispatcher implementation, not in a different interface surface.

### IDispatcher

The top-level orchestrator interface. Coordinates all cache operations.

| Method | Description |
|--------|-------------|
| `initialize(config)` | Creates N block devices + N extent managers from PCI addresses; builds the P2P BAR1 ring and cold-read pool |
| `shutdown()` | Drains background writes, tears down the P2P ring, shuts down all drives |
| `populate(key, ipc_handle)` | Single-call put: GPU→DRAM DMA, registers entry, enqueues write-through |
| `reserve_memory(key, size, session_id)` | Reserve a DRAM slot without DMA (returns raw pointer); `session_id` is an opaque per-request id (0 = unset) for observability only |
| `copy_gpu_to_memory_async(key, regions, stream)` | Async D2H copy from the client GPU region(s) into a previously reserved slot, on the given stream |
| `copy_gpu_to_memory_completed(key, size)` | Finalize the reserved slot: register in dispatch-map (`MemoryTier`) + enqueue write-through |
| `release_memory(key)` | Cancel a reserved slot without populating |
| `batch_populate(entries)` | Batch put across multiple keys |
| `lookup(key, ipc_handle)` | Serves from DRAM (warm) or via the P2P BAR1 cold path (SSD→GPU) |
| `lookup_async(key, ipc_handle)` | Non-blocking lookup, returns CUDA stream |
| `batch_lookup(entries)` | Batch lookup; `entries: &[(CacheKey, Vec<IpcHandle>)]`. **P2P cold path handles single-region entries only** — a multi-region entry is rejected with `InvalidParameter` |
| `check(key)` | Existence check without data transfer |
| `remove(key)` | Removes entry from all tiers |
| `touch(key)` | Refreshes eviction timestamp |
| `pin(key)` / `unpin(key)` | Hold/release an entry against eviction |
| `promote_to_memory_tier(keys)` | Pre-promote cold entries to DRAM for future warm access |
| `read_write_stats()` | Aggregate I/O counters (bytes/ops read and written) |
| `tier_event_stats()` | Tier-transition counters (promotions, demotions, backfills) |
| `clear_memory_tier()` | Evicts all DRAM entries |
| `flush_to_ssd()` | Force all pending write-through jobs to complete |

> The legacy `prepare_store` / `commit_store` / `cancel_store` direct-write API and
> the separate `populate_memory` / `memory_populated` split-populate names no longer
> exist. The current split-populate path is
> `reserve_memory` → `copy_gpu_to_memory_async` → `copy_gpu_to_memory_completed`
> (with `release_memory` as the cancellation path).

### IDispatchMap

The key→location index with reader/writer reference counting.

| Method | Description |
|--------|-------------|
| `initialize()` | Initializes internal state |
| `create_memory_tier_entry(key, ptr, size)` | Registers a DRAM-resident entry |
| `lookup(key)` | Returns a `LookupResult` (see enum below) |
| `convert_to_storage(key, offset)` | Records the `ssd_offset` for an entry; the entry **stays `MemoryTier`** (this only marks it durable/evictable, it does not itself demote) |
| `convert_memory_tier_to_block(key)` | Demotes a DRAM entry to SSD-only (`MemoryTier` → `BlockDevice`) |
| `promote_block_to_memory_tier(key, ptr, size)` | In-place flip `BlockDevice` → `MemoryTier`, retaining the existing `ssd_offset`; used by the DRAM backfill worker (works on pinned entries) |
| `try_evict_to_block(key)` | Demote `MemoryTier` → `BlockDevice`, only when `ssd_offset` is set and the entry is unreferenced |
| `take_read/release_read` | Reference counting for concurrent access |
| `take_write/release_write` | Exclusive writer semantics |
| `downgrade_reference(key)` | Atomically downgrade write ref to read ref |
| `remove(key)` | Removes entry (fails if active references) |
| `touch(key)` | Refreshes eviction ordering |
| `entry_size(key)` | Returns the size of a stored entry |
| `oldest_keys(n)` | Returns N oldest keys for eviction |
| `is_evictable(key)` | Checks if safe to evict (write-through complete, no refs) |
| `recover_extent(key, offset, size)` | Rebuilds from persisted extents on startup |

```rust
enum LookupResult {
    NotExist,
    MismatchSize,
    BlockDevice { offset: u64 },
    MemoryTier { pointer: *mut u8, size: u32 },
}
```

There is no `Staging` variant — direct-write staging was removed. Reader/writer
reference counting uses blocking semantics with a 100 ms timeout; recovery
rebuilds the map from extent-manager metadata on startup.

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
| `telemetry_snapshot()` | Point-in-time counters (inserts, evictions, hits) |
| `clear()` | Remove all entries, return count evicted |

### IBlockDevice

NVMe block device with actor-model architecture.

| Method | Description |
|--------|-------------|
| `connect_client()` | Returns (command_tx, completion_rx) channel pair |
| `sector_size(ns_id)` / `num_sectors(ns_id)` | Namespace geometry |
| `block_size()` | Device block/sector size |
| `max_transfer_size()` | MDTS limit |
| `max_queue_depth()` / `num_io_queues()` | Queue geometry |
| `numa_node()` | Controller NUMA node |
| `nvme_version()` | Reported NVMe spec version |
| `telemetry()` / `read_write_stats()` | I/O counters (feature-gated telemetry) |

Commands are sent as enum variants including `ReadSync`, `WriteSync`, `ReadAsync`, `WriteAsync`, `WriteZeros`, `BatchSubmit`, `AbortOp`, `ControllerReset`, `FlushSync`, and namespace-management ops.

### IExtentManager

Persistent space allocator with crash-consistent metadata.

| Method | Description |
|--------|-------------|
| `format(params)` | Writes superblock and initializes regions |
| `initialize()` | Recovers state from existing superblock |
| `reserve_extent(key, size)` | Returns WriteHandle (publish or auto-abort on drop) |
| `get_extents()` / `for_each_extent(f)` | Enumerate committed extents (used for recovery) |
| `remove_extent(offset)` | Frees extent space |
| `checkpoint()` / `set_checkpoint_interval(d)` | Persists / schedules persistence of allocation state |
| `used_bytes() / capacity_bytes()` | Utilization metrics |
| `get_instance_id()` | Unique instance identifier |

### IGpuServices

CUDA GPU operations for DMA transfers, including the P2P mapping used by this profile.

| Method | Description |
|--------|-------------|
| `initialize()` | Loads CUDA runtime, discovers GPUs (compute 7.0+) |
| `dma_copy_to_host(gpu_ptr, dma_buf, size)` | GPU→DRAM synchronous copy |
| `dma_copy_to_device(dma_buf, gpu_ptr, size)` | DRAM→GPU synchronous copy |
| `memcpy_h2d_async(src, dst, size, stream)` | Async DRAM→GPU on stream |
| `memcpy_d2d_async(src, dst, size, stream)` | Async device-to-device copy (BAR1 ring → client VRAM) |
| `create_stream() / destroy_stream()` | CUDA stream management |
| `stream_synchronize(stream)` | Block until stream completes |
| `allocate_pinned_dma_buffer(size)` | CUDA-pinned + SPDK-registered buffer |
| `register_host_memory(ptr, size)` | Pin existing memory for zero-copy |

### ISPDKEnv

SPDK environment initialization and lifecycle. In this profile it also registers GPU BAR1 memory with SPDK so the NVMe controller can DMA directly into it.

### ILogger

Simple logging interface (`error`, `warn`, `info`, `debug`).

## 5. Key Components in Detail

### 5.1 P2P Dispatcher (`components/dispatcher-p2p/`)

`DispatcherP2pComponent` is the central orchestrator for this profile. It shares
the standard dispatcher's put path, warm path, eviction, and miss-forwarding, and
replaces the cold path with a GPUDirect Storage pipeline.

It owns:

- **N data drives**: each a (BlockDevice, ExtentManager) pair created from PCI addresses
- **ParallelBackgroundWriter**: multi-threaded write-through (drains WriteJobs from a channel)
- **BackgroundEvictor**: monitors SSD utilization and reclaims extents
- **MemoryTierEvictor**: background DRAM→SSD demotion — when memory-tier utilization crosses a threshold, evictable `MemoryTier` entries with a recorded `ssd_offset` are flipped to `BlockDevice` (`try_evict_to_block`) to free DRAM without I/O
- **P2pRing**: the GPU BAR1 staging ring — GPU device memory exposed on the PCIe BAR, GDRCopy-mapped on the host and registered with SPDK, giving `P2P_RING_SLOTS = 64` slots the NVMe controller can DMA into directly
- **P2pColdReadPool**: a persistent per-drive worker pool (pre-connected NVMe channels + CUDA streams) that runs cold-read jobs through the BAR1 ring; also provides the DRAM-bounce fallback (`promote_and_serve`) when the ring is unavailable
- **DramBackfillWorker**: background thread that, after a P2P serve, asynchronously reads the block SSD→DRAM (after `backfill_delay_ms`) and calls `promote_block_to_memory_tier` to flip the entry to `MemoryTier`, so future lookups are warm
- **PipelineRing**: retained as the DRAM-bounce fallback path
- **Warm stream**: dedicated CUDA stream for async memory-tier→GPU copies (lock-free via AtomicU64)
- **BlockDeviceFactory / ExtentManagerFactory**: factory closures for creating DataDrive components during initialization

Receptacles: `dispatch_map`, `memory_tier`, `gpu_services`, `spdk_env`, `logger`, `remote_lookup`.

Key design decisions:
- Drive selection is `key % num_drives` (deterministic sharding)
- The memory-tier pool is registered with both CUDA (`cudaHostRegister`) and SPDK (`spdk_mem_register`); the GPU BAR1 ring is separately GDRCopy-mapped and SPDK-registered
- Write-through uses a read reference (not write) so lookups can proceed concurrently
- The P2P cold serve **keeps the entry `BlockDevice`**; promotion to DRAM is deferred to the backfill worker

### 5.2 Memory-Tier (`components/memory-tier/`)

DRAM cache pool using:
- **mmap'd contiguous allocation** for the pool
- **First-fit free-list allocator** with 4 KiB alignment (`allocator.rs`)
- **Delegated eviction ordering** via a bound `IEvictionPolicy` component (currently `eviction-policy-lru`)
- **HashMap<CacheKey, Slot>** for O(1) key lookup

Default pool size: 256 MiB. The pool is CUDA-pinned at server startup for zero-copy GPU DMA.

### 5.3 Dispatch Map (`components/dispatch-map/`)

The index tracking every cached entry's location and state. Receptacles: `eviction_policy` (IEvictionPolicy), `logger` (ILogger). Provides reader/writer reference counting with blocking semantics (100 ms timeout) and rebuilds from extent-manager metadata on recovery. See the `LookupResult` enum in §4.

### 5.4 Block Device (`components/block-device-spdk-nvme/`)

High-performance NVMe driver using SPDK userspace I/O:

- **Actor model**: dedicated thread per NVMe controller, NUMA-pinned
- **Channel-based I/O**: clients get SPSC channel pairs (command_tx, completion_rx)
- **Zero-copy**: DMA buffers from SPDK hugepages — in this profile the DMA target can be a GPU BAR1 ring slot, not just host DRAM
- **MDTS-aware**: I/O segmenter splits transfers exceeding the device's maximum data transfer size
- **Multiple queue depths**: exploits different NVMe I/O queues for varying batch sizes

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
- **Double-buffered checkpoints**: two copies for atomic updates (active_copy toggles)
- **Buddy allocator**: power-of-two extent sizes within each region
- **Regions**: configurable count (power of two), each independently managed
- **WriteHandle**: RAII pattern — `publish()` commits, `Drop` auto-aborts
- **Background checkpoint thread**: periodic persistence (default: 30 seconds)

### 5.6 GPU Services (`components/gpu-services/`)

CUDA integration layer providing:
- Device discovery and initialization (requires compute capability 7.0+)
- IPC memory handle management (open/close cross-process GPU memory)
- Synchronous and asynchronous DMA transfers (H2D, D2H, and device-to-device for the P2P path)
- CUDA-pinned host memory allocation and registration
- SPDK memory registration for NVMe DMA compatibility, including GPU BAR1 memory for GPUDirect Storage

## 6. Server and Client Architecture

### certus-server (`apps/certus-server/`)

A shared-memory-queue (shmq) server exposing the `IDispatcher` interface over a
`/dev/shm` mailbox file. There is no TCP port and no network transport; clients
reach the server by sharing the host IPC namespace and `/dev/shm` (podman
`--ipc=host` or k8s `hostIPC: true`).

**CLI options:**
- `--device-pci` — NVMe PCI addresses (repeatable)
- `--device-path` — Filesystem device path (alternative to PCI, e.g., `/dev/null` for testing)
- `--shm-path` — Shared-memory mailbox file path (default: `/dev/shm/certus-shmq`)
- `--channels` — Number of mailbox channels (= max in-flight requests = worker threads; e.g. `32`)
- `--memory-tier-size` — Pool size (e.g., `256M`, `1G`, `4G`)
- `--format` — Format SSD extents on startup (start fresh)

**Startup sequence** (`full-p2p` init order):
1. Logger
2. SPDK environment initialization
3. GPU services initialization
4. Eviction policy (LRU)
5. Dispatch map creation
6. Memory-tier allocation + CUDA host registration
7. Remote-lookup (placeholder)
8. **DispatcherP2p** initialization — creates block devices + extent managers, builds the P2P BAR1 ring and cold-read pool, starts background workers

**shmq ops** (opcode-framed binary wire: `lib/shmq-dispatcher/src/wire.rs`):
- `Populate`, `Lookup`, `Check`, `Remove`, `Touch`, `ClearMemoryTier`, `FlushToSsd`, `GetIoStats`
- The remaining `IDispatcher` surface (Reserve, CopyToMemory, Pin, Unpin, TakeEvents, …) is carried as additional opcodes; all operations accept batches and use CUDA IPC handles (64-byte `cudaIpcMemHandle_t`)

### certus-connector (`certus-connector/`)

A PyO3 native module (`certus_native`) for direct Python integration with vLLM: embeds the full Certus component stack in-process and exposes a `CertusEngine` Python class.

## 7. Concurrency Model

### Reference Counting (Dispatch Map)

- **Populate**: acquires write ref → downgrades to read ref for background writer
- **Lookup**: acquires read ref → released after DMA copy
- **P2P cold serve**: holds a read pin on the `BlockDevice` entry for the duration of the SSD→BAR1→GPU pipeline
- **Remove**: blocks if write ref active; fails if read refs active
- **Eviction**: skips entries with active references

### Background Threads

| Thread | Purpose |
|--------|---------|
| `dispatcher-bg-writer` | Parallel write-through: memory-tier → SSD |
| `dispatcher-bg-evictor` | Monitors SSD utilization, reclaims extents |
| `dispatcher-bg-mt-evictor` | DRAM→SSD demotion: flips evictable `MemoryTier` entries to `BlockDevice` under memory pressure |
| `dispatcher-bg-backfill` | P2P: after a P2P cold serve, asynchronously reads SSD→DRAM and promotes the entry to `MemoryTier` |
| P2P cold-read pool workers | Per-drive workers running SSD→BAR1→GPU cold reads |
| `extent-mgr-checkpoint` | Periodic checkpoint of allocation metadata |
| Block device actor threads | One per NVMe controller, NUMA-pinned |

### P2P Cold-Path Ring (Cold-Path Optimization)

For SSD→GPU transfers the profile uses the GPU BAR1 staging ring (`P2P_RING_SLOTS = 64`):

1. A cold-pool worker issues chunked NVMe reads (MDTS granularity) that DMA directly into free BAR1 ring slots
2. On each completion, a device-to-device copy moves the slot into the client's destination VRAM
3. Successive chunks pipeline: while one slot streams D2D to the client, the next NVMe read fills another slot
4. Result: NVMe I/O and GPU D2D copies overlap, with **no host DRAM bounce** — a single PCIe hop from SSD to GPU

## 8. Eviction Policies

### DRAM Eviction

- **Trigger**: memory-tier pool full during populate/reserve, or the background `MemoryTierEvictor` crossing a utilization threshold
- **Policy**: LRU (oldest entries evicted first)
- **Eligibility**: write-through must be complete (`ssd_offset` set) and no active references
- **Outcome**: dispatch-map entry transitions `MemoryTier` → `BlockDevice` (via `try_evict_to_block` / `convert_memory_tier_to_block`)
- **Fallback**: under extreme pressure, blind LRU eviction (potential data loss, acceptable for cache)

### SSD Eviction (Background Evictor)

- **Trigger**: SSD utilization exceeds threshold (default: 90%)
- **Target**: low-watermark (default: 80%)
- **Policy**: oldest keys from dispatch-map
- **Batch size**: configurable (default: 64 extents per sweep)
- **Interval**: configurable (default: 5 seconds between checks)
- **Outcome**: extent freed, entry removed from dispatch-map entirely

## 9. Crash Recovery

Certus operates under **cache semantics** — data loss on crash is acceptable because the source of truth lives elsewhere (e.g., GPU recomputation).

On restart:
1. Memory-tier (DRAM) is empty — all volatile data is lost
2. Dispatch-map is rebuilt by iterating finalized extents from each extent manager
3. Non-finalized extents (incomplete writes) are reclaimed as free space
4. System resumes with only SSD-persisted entries visible (as `BlockDevice`)

## 10. Build System

### Full Build (requires SPDK)

```bash
cargo build --workspace          # All members including SPDK-dependent crates
cargo build -p dispatcher-p2p    # The P2P dispatcher used by this profile
cargo build -p certus-server-yaml
```

### Feature Gates

| Feature | Crate | Effect |
|---------|-------|--------|
| `spdk` | interfaces | Enables SPDK-dependent interface traits and types |
| `gpu` | gpu-services | Enables real CUDA FFI (vs. stub), including P2P/BAR1 mapping |
| `telemetry` | block-device-spdk-nvme | Enables I/O latency/throughput collection |
| `hardware-test` | dispatcher-p2p | Enables integration tests requiring real NVMe + GPU |
| `testing` | extent-manager | Exposes test utilities and superblock internals |

### SPDK Dependencies

SPDK must be pre-built at `deps/spdk-build/`. Requires:
- Kernel boot params: IOMMU + 1G hugepages
- `memlock` set to unlimited
- NVMe devices bound to `vfio-pci`
- GPUDirect Storage support (GDRCopy) for the P2P cold path

## 11. Testing Strategy

- **Unit tests**: mocked dependencies (MockDispatchMap, MockMemoryTier, MockGpuServices)
- **Integration tests**: full component wiring without hardware
- **Hardware tests**: feature-gated (`--features hardware-test`), require real NVMe + GPU
- **Benchmarks**: Criterion-based; `apps/gpu-bb-vs-p2p/` compares the DRAM bounce-buffer path against the P2P path
- **CI**: GitHub Actions on `ubuntu-latest`, default members only, single-threaded

## 12. Key Design Decisions

1. **Cache, not storage**: no durability guarantees. Crash loses in-flight data. Source of truth is external.

2. **GPUDirect P2P cold path** (the defining choice of this profile): SSD→GPU cold reads bypass the DRAM bounce buffer. The NVMe controller DMAs into a GPU BAR1 staging ring (GDRCopy-mapped, SPDK-registered), then a device-to-device copy delivers the data into the client's VRAM — one PCIe hop instead of two. The served entry stays `BlockDevice`; a background `DramBackfillWorker` promotes it to DRAM afterward so repeat lookups go warm. The P2P path serves single-region blocks only; multi-region lookups fall back / are rejected.

3. **Write-through, not write-back**: data persists to SSD asynchronously after acknowledgement. Reads can be served from DRAM immediately.

4. **Component isolation**: each component has its own CLAUDE.md, tests, and benchmarks. LLM context stays small by scoping to one component + its interface bindings.

5. **Deterministic sharding**: `key % num_drives` selects the target SSD. Simple, no coordination needed for single-process deployment.

6. **Zero-copy pipeline**: the memory-tier pool is simultaneously CUDA-pinned and SPDK-registered; the GPU BAR1 ring is GDRCopy-mapped and SPDK-registered — enabling direct NVMe reads into GPU-accessible memory without intermediate host copies.
