# Certus System Architecture

This document describes the architecture of the Certus storage system as a developer reference.

## 1. What is Certus?

Certus is a **generative domain-specific storage system for AI inferencing workloads**. It functions as a GPU KV-cache offloading engine: tensor data produced during inference is cached in DRAM and persisted to NVMe SSDs, then served back to GPU memory on demand via zero-copy DMA transfers.

The system sits between GPU inference engines (e.g., vLLM) and NVMe storage, providing:

- **Sub-millisecond warm lookups** — GPU←DRAM via CUDA async memcpy
- **Pipelined cold reads** — NVMe→DRAM→GPU with overlapped I/O and DMA
- **Write-through persistence** — DRAM cache with background SSD write-through
- **Pluggable eviction** — Two-tier eviction (DRAM pool and SSD capacity)

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
2. Pipelined NVMe reads into memory-tier slot (8-deep ring buffer)
3. Simultaneously streams chunks to GPU via async CUDA DMA
4. Entry promoted back to memory-tier for future warm access

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
            data_drives: RwLock<Vec<DataDrive>>,
            pending_writes: Mutex<HashMap<CacheKey, PendingWrite>>,
            pipeline_ring: RwLock<Option<PipelineRing>>,
            warm_stream: AtomicU64,
            block_device_factory: Mutex<Option<BlockDeviceFactory>>,
            extent_manager_factory: Mutex<Option<ExtentManagerFactory>>,
            // ...
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
components/
├── component-framework/     # Core framework (3 crates)
│   └── crates/
│       ├── component-core/  # Traits, actor, channels, NUMA
│       ├── component-macros/# Proc macros (define_interface!, define_component!)
│       └── component-framework/ # Facade re-export
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
├── remote-lookup/           # Remote-cache orchestrator (IRemoteLookup)
├── remote-lookup-rdma-initiator/  # Outbound RDMA push / data-holder side (IRemoteLookupRdmaInitiator)
├── remote-lookup-rdma-responder/  # Passive RDMA accept side / requester (IRemoteLookupRdmaResponder[Admin])
├── zyre/                    # Gossip/beacon peer discovery (IZyre)
├── spdk-env/                # SPDK environment wrapper (ISPDKEnv)
├── spdk-sys/                # Raw FFI bindings to SPDK
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
└── helloworld-mainline/     # Framework example app
certus-connector/            # PyO3 module for vLLM integration
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
| `reserve_memory(key, size, session_id)` | Reserve a DRAM slot without DMA (returns raw pointer); `session_id` is an opaque per-request id (0 = unset) for observability only |
| `populate_memory(key, ipc_handle)` | DMA into a previously reserved slot |
| `memory_populated(key, size)` | Finalize reserved slot: register in dispatch-map + enqueue write-through |
| `release_memory(key)` | Cancel a reserved slot without populating |
| `lookup(key, ipc_handle)` | Serves from DRAM (warm) or promotes from SSD (cold) |
| `lookup_async(key, ipc_handle)` | Non-blocking lookup, returns CUDA stream |
| `batch_lookup(entries)` | Batch lookup: multiple keys with parallel SSD promotion |
| `check(key)` | Existence check without data transfer |
| `remove(key)` | Removes entry from all tiers |
| `touch(key)` | Refreshes eviction timestamp |
| `promote_to_memory_tier(keys)` | Pre-promote cold entries to DRAM for future warm access |
| `prepare_store(key, size)` | Direct-write API: allocates DMA buffer + extent |
| `commit_store(key)` | Writes prepared buffer to SSD |
| `cancel_store(key)` | Aborts prepared write |
| `clear_memory_tier()` | Evicts all DRAM entries |
| `flush_to_ssd()` | Force all pending write-through jobs to complete |

### IDispatchMap

The key→location index with reader/writer reference counting.

| Method | Description |
|--------|-------------|
| `set_dma_alloc(alloc)` | Registers the DMA buffer allocator function |
| `initialize()` | Initializes internal state |
| `create_staging(key, size)` | Allocates DMA staging buffer for a key |
| `create_memory_tier_entry(key, ptr, size)` | Registers DRAM-resident entry |
| `lookup(key)` | Returns `LookupResult` enum (NotExist/Staging/MemoryTier/BlockDevice) |
| `convert_to_storage(key, offset)` | Transitions entry to SSD-backed state |
| `convert_memory_tier_to_block(key)` | Demotes DRAM entry to SSD-only |
| `take_read/release_read` | Reference counting for concurrent access |
| `take_write/release_write` | Exclusive writer semantics |
| `downgrade_reference(key)` | Atomically downgrade write ref to read ref |
| `remove(key)` | Removes entry (fails if active references) |
| `touch(key)` | Refreshes eviction ordering |
| `entry_size(key)` | Returns the size of a stored entry |
| `oldest_keys(n)` | Returns N oldest keys for eviction |
| `is_evictable(key)` | Checks if safe to evict (write-through complete, no refs) |
| `recover_extent(key, offset, size)` | Rebuilds from persisted extents on startup |

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
| `clear()` | Remove all entries, return count evicted |

### IBlockDevice

NVMe block device with actor-model architecture.

| Method | Description |
|--------|-------------|
| `connect_client()` | Returns (command_tx, completion_rx) channel pair |
| `block_size()` | Device sector size |
| `num_sectors(ns_id)` | Namespace capacity |
| `max_transfer_size()` | MDTS limit |
| `numa_node()` | Controller NUMA node |

Commands are sent as enum variants: `ReadAsync`, `WriteSync`, `ReadSync`, `WriteZeros`.

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

Both variants own:

- **N data drives**: Each is a (BlockDevice, ExtentManager) pair created from PCI addresses
- **ParallelBackgroundWriter**: Multi-threaded write-through (drains jobs from a channel)
- **BackgroundEvictor**: Monitors SSD utilization and reclaims space
- **Pipeline ring**: Pre-allocated ring of 8 CUDA-pinned DMA buffers + 2 CUDA streams for pipelined SSD→GPU reads
- **Warm stream**: Dedicated CUDA stream for async memory-tier→GPU copies (lock-free access via AtomicU64)
- **BlockDeviceFactory / ExtentManagerFactory**: Factory closures for creating DataDrive components during initialization

The P2P variant additionally owns:
- **P2pRing**: GPUDirect Storage ring for direct SSD→GPU transfers
- **DramBackfillWorker**: Background thread that asynchronously promotes hot P2P entries back to DRAM

Receptacles: `dispatch_map`, `memory_tier`, `gpu_services`, `spdk_env`, `logger`, `remote_lookup`.

Key design decisions:
- Drive selection is `key % num_drives` (deterministic sharding)
- Memory-tier pool is registered with both CUDA (`cudaHostRegister`) and SPDK (`spdk_mem_register`) for zero-copy transfers in both directions
- Write-through uses a read reference (not write) so lookups can proceed concurrently

### 5.2 Memory-Tier (`components/memory-tier/`)

DRAM cache pool using:
- **mmap'd contiguous allocation** for the pool
- **First-fit free-list allocator** with 4 KiB alignment (`allocator.rs`)
- **Delegated eviction ordering** via a bound `IEvictionPolicy` component (currently `eviction-policy-lru`)
- **HashMap<CacheKey, Slot>** for O(1) key lookup

Default pool size: 256 MiB. The pool is CUDA-pinned at server startup for zero-copy GPU DMA.

### 5.3 Dispatch Map (`components/dispatch-map/`)

The index tracking every cached entry's location and state. Receptacles: `eviction_policy` (IEvictionPolicy), `extent_manager` (IExtentManager), `logger` (ILogger).

```rust
enum LookupResult {
    NotExist,
    MismatchSize,
    Staging { buffer: Arc<DmaBuffer> },
    BlockDevice { offset: u64 },
    MemoryTier { pointer: *mut u8, size: u32 },
}
```

Provides reader/writer reference counting with blocking semantics (writers block readers and vice versa, with 100ms timeout). Supports recovery by rebuilding from extent manager metadata.

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

**CLI options:**
- `--device-pci` — NVMe PCI addresses (repeatable)
- `--device-path` — Filesystem device path (alternative to PCI, e.g., `/dev/null` for testing)
- `--shm-path` — Shared-memory mailbox file path (default: `/dev/shm/certus-shmq`)
- `--channels` — Number of mailbox channels (= max in-flight requests = worker threads; e.g. `32`)
- `--memory-tier-size` — Pool size (e.g., `256M`, `1G`, `4G`)
- `--format` — Format SSD extents on startup (start fresh)

**Startup sequence:**
1. SPDK environment initialization
2. GPU services initialization
3. Dispatch map creation
4. Memory-tier allocation + CUDA host registration
5. Dispatcher initialization (creates block devices + extent managers)
6. shmq mailbox creation + poller/worker start

**shmq ops** (opcode-framed binary wire: `components/shmq-dispatcher/src/wire.rs`):
- `Populate` — Batch insert from GPU memory
- `Lookup` — Batch retrieve to GPU memory
- `Check` — Batch existence check
- `Remove` — Batch delete
- `Touch` — Batch LRU refresh
- `ClearMemoryTier` — Evict all DRAM entries
- `FlushToSsd` / `GetIoStats` — flush write-through / read I/O counters

The full `IDispatcher` surface (Reserve, CopyToStore, CommitStore, AbortStore,
Pin, Unpin, TakeEvents, …) is carried as additional opcodes. All operations
accept batches and use CUDA IPC handles (64-byte `cudaIpcMemHandle_t`) for
cross-process GPU memory sharing (the shared IPC namespace lets the server open
the client's handles).

### certus-connector (`certus-connector/`)

A PyO3 native module (`certus_native`) for direct Python integration with vLLM:
- Embeds the full Certus component stack in-process
- Exposes `CertusEngine` Python class
- Operations: `batch_check`, `touch`, `prepare_store`, `complete_store`, `prepare_load`, `complete_load`, `store_async`, `load_async`, `poll_completions`

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
| `dispatcher-bg-backfill` | (P2P variant) DRAM backfill for hot P2P entries |
| `extent-mgr-checkpoint` | Periodic checkpoint of allocation metadata |
| Block device actor threads | One per NVMe controller, NUMA-pinned |

### Pipeline Ring (Cold-Path Optimization)

For SSD→GPU transfers, the system uses a pre-allocated ring of 8 DMA buffers with 2 CUDA streams:

1. Prime ring with async NVMe reads
2. On each completion: copy chunk to memory-tier, queue async H2D transfer
3. Overlap: while stream[0] copies to GPU, stream[1] receives next chunk
4. Result: NVMe I/O and GPU DMA run in parallel

## 8. Eviction Policies

### DRAM Eviction

- **Trigger**: Memory-tier pool full during populate
- **Policy**: LRU (oldest entries evicted first)
- **Eligibility**: Write-through must be complete (ssd_offset set) and no active references
- **Outcome**: Dispatch-map entry transitions MemoryTier → BlockDevice
- **Fallback**: Under extreme pressure, blind LRU eviction (potential data loss, acceptable for cache)

### SSD Eviction (Background Evictor)

- **Trigger**: SSD utilization exceeds threshold (default: 90%)
- **Target**: Low-watermark (default: 80%)
- **Policy**: Oldest keys from dispatch-map
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

5. **Deterministic sharding**: `key % num_drives` selects the target SSD. Simple, no coordination needed for single-process deployment.

6. **Zero-copy pipeline**: Memory-tier pool is simultaneously CUDA-pinned and SPDK-registered, enabling direct NVMe reads into GPU-accessible memory without intermediate copies.
