# Certus Server — Component Deployment Diagram

## Component Topology

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ Client Process                                                               │
│                                                                              │
│  ┌──────────────────────────────────────────┐                                │
│  │ Python Test Client                       │       ┌───────────────────┐    │
│  │  • PyTorch GPU allocation                │──────▶│ GPU Memory        │    │
│  │  • cudaIpcGetMemHandle (64-byte handle)  │       │ (client context)  │    │
│  │  • Batch shmq requests                   │       └─────────┬─────────┘    │
│  └──────────────────┬───────────────────────┘                 │              │
│                     │                                         │ CUDA IPC     │
└─────────────────────┼─────────────────────────────────────────┼──────────────┘
                      │ shmq (/dev/shm mailbox, shared IPC)     │
                      ▼                                         ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│ certus-server Process                                                        │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ shmq translate/serve layer (shmq-dispatcher)                           │  │
│  │  • Populate / Lookup / Check / Remove / Touch shmq ops                 │  │
│  │  • cudaIpcOpenMemHandle / cudaIpcCloseMemHandle (cached per batch)     │  │
│  │  • Batch→singular mapping, duplicate-key rejection                     │  │
│  └────────────────────────────────┬───────────────────────────────────────┘  │
│                                   │                                          │
│  ┌────────────────────────────────▼───────────────────────────────────────┐  │
│  │ DispatcherComponent                              «IDispatcher»         │  │
│  │  (--dispatcher-version v0|v1)                                          │  │
│  │                                                                        │  │
│  │  receptacles:                                                          │  │
│  │    ├─ dispatch_map ──────────────────┐                                 │  │
│  │    ├─ memory_tier ────────┐          │                                 │  │
│  │    ├─ gpu_services ───────┼──┐       │                                 │  │
│  │    ├─ spdk_env ───┐       │  │       │                                 │  │
│  │    └─ logger ─┐   │       │  │       │                                 │  │
│  │               │   │       │  │       │                                 │  │
│  │  ┌────────────┼───┼───────┼──┼───────┼─────────────────────────────┐   │  │
│  │  │ Inner: DataDrive[0..N] │  │(one per│--device-pci address)       │   │  │
│  │  │            │   │       │  │       │                             │   │  │
│  │  │  ┌─────────┼───┼───────┼──┼─┐  ┌─────┼─────────────────────────┐   │  │
│  │  │  │ BlockDeviceSpdkNvme   │ │  │ ExtentManager               │   │  │
│  │  │  │ «IBlockDevice»         │ │  │ «IExtentManager»              │   │  │
│  │  │  │ «IBlockDeviceAdmin»    │ │  │  receptacles:                 │   │  │
│  │  │  │  receptacles:          │ │  │    ├─ metadata_device ──▶[BD] │   │  │
│  │  │  │    ├─ spdk_env         │ │  │    └─ logger                  │   │  │
│  │  │  │    └─ logger           │ │  └───────────────────────────────┘   │  │
│  │  │  └────────────────────────┘ │                                      │  │
│  │  └─────────────────────────────┼──────────────────────────────────────┘  │
│  │               │   │       │  │       │                                 │  │
│  │  [BackgroundWriter] ──────┼──┼───────┼── WriteJob ──▶ DataDrive        │  │
│  │  [BackgroundEvictor] ─────┼──┼───────┼── Evict stale SSD extents       │  │
│  │  [PipelineRing] ──────────┘  │       │── Pipelined SSD→DRAM→GPU reads  │  │
│  └───────────────┼───┼──────────┼───────┼─────────────────────────────────┘  │
│                  │   │       │  │       │                                    │
│                  ▼   │       │  ▼       ▼                                    │
│  ┌────────────────┐  │  ┌─────────┐  ┌────────────────────────────────────┐  │
│  │ LoggerComponent   │  │  │MemoryTier│  │ DispatchMapComponent             │  │
│  │ «ILogger»      │  │  │Component│  │ «IDispatchMap»                     │  │
│  │                │  │  │«IMemory │  │                                    │  │
│  │ (all comps     │  │  │ Tier»   │  │  receptacles:                      │  │
│  │  bind here)    │  │  │         │  │    ├─ extent_manager ──┐           │  │
│  └────────────────┘  │  │ • mmap  │  │    └─ logger           │           │  │
│                      │  │   pool  │  │                        │           │  │
│                      │  │ • LRU   │  │  [Entry Table]         │           │  │
│                      │  │ • first │  │   key → {MemoryTier|   │           │  │
│                      │  │   -fit  │  │     BlockDevice|Staging}│           │  │
│                      │  │ alloc.  │  └────────────────────────┼───────────┘  │
│                      │  │         │                           │              │
│                      │  │ recepts:│                           ▼              │
│                      │  │  └logger│  ┌────────────────────────────────────┐  │
│                      │  └─────────┘  │ Metadata ExtentManager           │  │
│                      │               │ «IExtentManager»                   │  │
│                      │               │  receptacles:                      │  │
│                      │               │    ├─ metadata_device ─┐           │  │
│                      │               │    └─ logger           │           │  │
│                      │               └────────────────────────┼───────────┘  │
│                      │                                        │              │
│                      │                                        ▼              │
│                      │               ┌────────────────────────────────────┐  │
│                      │               │ Metadata BlockDeviceSpdkNvme     │  │
│                      │               │ «IBlockDevice, IBlockDeviceAdmin»  │  │
│                      │               │  receptacles:                      │  │
│                      │               │    ├─ spdk_env ────┐               │  │
│                      │               │    └─ logger       │               │  │
│                      │               └────────────────────┼───────────────┘  │
│                      │                                    │                  │
│                      ▼                                    ▼                  │
│  ┌───────────────────────────────────────────────────────────┐               │
│  │ SPDKEnvComponent                          «ISPDKEnv»      │               │
│  │  • DPDK/EAL initialization                                │               │
│  │  • VFIO device discovery & probing                        │               │
│  │  • Hugepage memory management                             │               │
│  └───────────────────────┬───────────────────────────────────┘               │
│                          │                                                   │
│  ┌───────────────────────────────────────────────────────────┐               │
│  │ GpuServicesComponent                    «IGpuServices»  │               │
│  │  receptacles:                                             │               │
│  │    └─ logger                                              │               │
│  │  • cudaMemcpy (H2D / D2H, sync + async streams)          │─▶ GPU Memory  │
│  │  • DMA buffer management (CUDA-pinned)                    │  (server ctx) │
│  │  • cudaHostRegister for memory-tier zero-copy             │               │
│  └───────────────────────────────────────────────────────────┘               │
│                          │                                                   │
│                          ▼ VFIO                                              │
│              ┌───────────────────────────────────────────────┐            │
│              │  NVMe device(s) [0..N]                         │            │
│              │  --device-pci (data + metadata co-located)     │            │
│              └───────────────────────────────────────────────┘            │
└──────────────────────────────────────────────────────────────────────────────┘
```

## Component Summary

| Component | Provides | Receptacles |
|-----------|----------|-------------|
| SPDKEnvComponent | ISPDKEnv | — |
| LoggerComponent | ILogger | — |
| GpuServicesComponent | IGpuServices | logger |
| EvictionPolicyLruComponent | IEvictionPolicy | logger |
| MemoryTierComponent | IMemoryTier | logger, eviction_policy |
| RemoteLookupComponent | IRemoteLookup | logger |
| BlockDeviceSpdkNvme | IBlockDevice, IBlockDeviceAdmin | spdk_env, logger |
| ExtentManager | IExtentManager | metadata_device, logger |
| DispatchMapComponent | IDispatchMap | extent_manager, eviction_policy, logger |
| DispatcherComponent | IDispatcher | dispatch_map, memory_tier, gpu_services, spdk_env, logger, remote_lookup |

## Initialization Order

1. **SPDKEnvComponent** — DPDK/EAL init, VFIO device discovery
2. **LoggerComponent** — console/file logging
3. **GpuServicesComponent** — CUDA device init
4. **EvictionPolicyLruComponent** — LRU eviction policy (shared by dispatch-map and memory-tier)
5. **DispatchMapComponent** — key→location table (bound to eviction_policy, extent_manager)
6. **MemoryTierComponent** — mmap DRAM pool, CUDA-pinned via `cudaHostRegister` (bound to eviction_policy)
7. **RemoteLookupComponent** — remote cache cluster integration
8. **DispatcherComponent** — top-level orchestrator (bound to dispatch_map, memory_tier, gpu, spdk_env, remote_lookup)
   - Internally creates **DataDrive[0..N]**: one (BlockDeviceSpdkNvme + ExtentManager) pair per `--device-pci` address; each drive stores both data and metadata
   - Allocates **PipelineRing** (CUDA-pinned + SPDK-registered ring buffers) for pipelined reads
   - Creates dedicated **warm_stream** (CUDA stream) for async memory-tier→GPU DMA
   - Starts **ParallelBackgroundWriter** for async DRAM→SSD write-through
   - Starts **BackgroundEvictor** thread for SSD-tier space reclamation (threshold-based)

## Data Flow

### Populate (GPU → DRAM → SSD)
```
Client GPU ──cudaIPC──▶ Server GPU ptr ──cudaMemcpy D2H──▶ Memory-tier DRAM slot
    ──BackgroundWriter (async write-through)──▶ NVMe data drive (via extent manager)
```

Entry registered in dispatch-map as `MemoryTier` immediately (available for lookup).
Background writer persists to SSD and sets `ssd_offset` for durability.

### Lookup — Warm Path (DRAM → GPU)
```
Memory-tier DRAM slot ──cudaMemcpyAsync H2D (warm_stream)──▶ Server GPU ptr
    ──cudaIPC──▶ Client GPU
```

LRU timestamp refreshed on hit. Zero-copy from pinned memory-tier pool.

### Lookup — Cold Path (SSD → DRAM → GPU)
```
NVMe data drive ──PipelineRing (async chunked reads)──▶ Memory-tier DRAM slot
    ──cudaMemcpyAsync H2D──▶ Server GPU ptr ──cudaIPC──▶ Client GPU
```

Promotes entry back to memory-tier (updates dispatch-map MemoryTier location).
Evicts LRU entries if DRAM pool is full before insertion.

### Eviction (DRAM → SSD only)
```
Memory-tier full ──LRU evict──▶ Dispatch-map entry transitions from MemoryTier → BlockDevice
    (data already on SSD via write-through; only pointer/state change)
```

### SSD Eviction (BackgroundEvictor)
```
SSD usage > threshold ──oldest_keys scan──▶ Remove extents from SSD
    ──Frees blocks in extent manager──▶ Space available for new writes
```

## CLI Options

```
certus-server-yaml \
    --device-pci DDDD:BB:DD.F [--device-pci ...] \
    --memory-tier-size 4G \
    --format \
    --shm-path /dev/shm/certus-shmq \
    --channels 32
```

| Flag | Description |
|------|-------------|
| `--device-pci` | PCI address(es) of NVMe device(s), repeatable |
| `--device-path` | Filesystem device path (alternative to PCI, e.g., `/dev/null` for testing) |
| `--memory-tier-size` | DRAM pool size (e.g., `256M`, `1G`, `4G`) |
| `--format` | Format SSD extents on startup (start fresh) |
| `--shm-path` | Path to the shared-memory mailbox file (default `/dev/shm/certus-shmq`) |
| `--channels` | Number of mailbox channels (= max in-flight requests = worker threads) |

A client reaches the server by sharing the host IPC namespace and `/dev/shm`
(podman `--ipc=host`, or k8s `hostIPC: true` with a shared `/dev/shm`). The
shared `/dev/shm` mailbox path *is* the endpoint — there is no server address to
configure. `--ipc=host` does double duty: the host server opens the container's
CUDA IPC handles, and the container sees the host `/dev/shm` mailbox.

## shmq Ops (opcode-framed, see `lib/shmq-dispatcher/src/wire.rs`)

The transport is a small opcode-based binary framing carried in the `/dev/shm`
mailbox. Each op maps to an `IDispatcher` method.

| Op | Request | Response | Description |
|-----|---------|----------|-------------|
| Populate | HandleBatch | per-op status | GPU→DRAM→SSD cache insertion |
| Lookup | HandleBatch | per-op status | Serve from DRAM or promote from SSD→GPU |
| Check | key list | per-key existence | Existence check (no data transfer) |
| Remove | key list | per-key status | Evict from DRAM + SSD, free extents |
| Touch | key list | per-key status | Refresh LRU timestamp without DMA |
| ClearMemoryTier | (empty) | entries_cleared | Evict all DRAM entries |
| FlushToSsd | (empty) | jobs_flushed | Force pending write-through to complete |
| GetIoStats | (empty) | I/O counters | Read/write op, byte, and latency totals |

## Notes

- The PlantUML source is in `certus-server-deployment.puml` for rendered diagrams
- To render: `plantuml certus-server-deployment.puml` (produces PNG/SVG)
- All component bindings use the COM-style `receptacle.connect(Arc<dyn Interface>)` pattern
- The Dispatcher internally instantiates and owns its DataDrive components (not externally wired)
- Memory-tier pool is registered with CUDA (`cudaHostRegister`) for pinned zero-copy DMA
- PipelineRing pre-allocates 8 CUDA-pinned SPDK-registered buffers to avoid per-request allocation
- Dispatcher variant is selected via YAML profile (e.g., `full`, `full-p2p`, `full-remote`)
- A `dispatcher-p2p` variant adds P2pRing and DramBackfillWorker for GPUDirect cold paths
