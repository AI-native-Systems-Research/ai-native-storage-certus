# Certus Server — Component Deployment Diagram (`full-p2p` Profile)

## Component Topology

The `full-p2p` composition is identical to `full` except the dispatcher is
`DispatcherP2pComponent` (crate `dispatcher-p2p`), which adds the **P2P BAR1
staging ring**, a **P2pColdReadPool**, a **DramBackfillWorker**, and a
**MemoryTierEvictor**, and serves cold reads directly from NVMe into GPU memory.

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
│                     │                        P2P (BAR1 ring)  │ + P2P D2D    │
└─────────────────────┼─────────────────────────────────────────┼──────────────┘
                      │ shmq (/dev/shm mailbox, shared IPC)     │
                      ▼                                         ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│ certus-server Process (full-p2p profile)                                     │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ shmq translate/serve layer (shmq-dispatcher)                           │  │
│  │  • Populate / Lookup / Check / Remove / Touch / Flush shmq ops         │  │
│  │  • cudaIpcOpenMemHandle / cudaIpcCloseMemHandle (cached per batch)     │  │
│  │  • Batch→singular mapping, duplicate-key rejection                     │  │
│  └────────────────────────────────┬───────────────────────────────────────┘  │
│                                   │                                          │
│  ┌────────────────────────────────▼───────────────────────────────────────┐  │
│  │ DispatcherP2pComponent                           «IDispatcher»        │  │
│  │  (crate: dispatcher-p2p)                                               │  │
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
│  │  │  ┌─────────┼───┼───────┼──┼─┐  ┌─────┼─────────────────────────┐   │  │
│  │  │  │ BlockDeviceSpdkNvme   │ │  │ ExtentManager               │   │  │
│  │  │  │ «IBlockDevice»         │ │  │ «IExtentManager»              │   │  │
│  │  │  └────────────────────────┘ │  └───────────────────────────────┘   │  │
│  │  └─────────────────────────────┼──────────────────────────────────────┘  │
│  │                                                                        │  │
│  │  Background & cold-path workers:                                       │  │
│  │  [ParallelBackgroundWriter] ── WriteJob ──▶ DataDrive                  │  │
│  │  [BackgroundEvictor] ───────── Evict stale SSD extents                 │  │
│  │  [MemoryTierEvictor] ───────── DRAM→SSD demotion (MemoryTier→Block)    │  │
│  │  [P2pColdReadPool] ─────────── SSD → BAR1 ring → D2D → client GPU      │  │
│  │  [DramBackfillWorker] ──────── async SSD → DRAM promote after P2P serve│  │
│  │  [P2pRing] ─────────────────── GPU BAR1 staging (64 slots, GDRCopy)    │  │
│  │  [PipelineRing] ────────────── DRAM-bounce cold fallback               │  │
│  └───────────────┼───┼──────────┼───────┼─────────────────────────────────┘  │
│                  │   │       │  │       │                                    │
│                  ▼   │       │  ▼       ▼                                    │
│  ┌────────────────┐  │  ┌─────────┐  ┌────────────────────────────────────┐  │
│  │ LoggerComponent   │  │  │MemoryTier│  │ DispatchMapComponent             │  │
│  │ «ILogger»      │  │  │Component│  │ «IDispatchMap»                     │  │
│  │                │  │  │«IMemory │  │   key → {MemoryTier | BlockDevice} │  │
│  │                │  │  │ Tier»   │  │  receptacles:                      │  │
│  │                │  │  │ • mmap  │  │    ├─ eviction_policy              │  │
│  │                │  │  │   pool  │  │    └─ logger                       │  │
│  │                │  │  │ • LRU   │  └────────────────────────────────────┘  │
│  └────────────────┘  │  └─────────┘                                          │
│                      │                                                        │
│                      ▼                                                        │
│  ┌───────────────────────────┐  ┌────────────────────────────────────────┐  │
│  │ GpuServicesComponent      │  │ EvictionPolicyLruComponent             │  │
│  │ «IGpuServices»            │  │ «IEvictionPolicy»                      │  │
│  │  • H2D / D2H / D2D copies │  │  receptacles: [logger]                 │  │
│  │  • BAR1 map + SPDK reg    │  └────────────────────────────────────────┘  │
│  │  receptacles: [logger]    │                                              │
│  └───────────────────────────┘  ┌────────────────────────────────────────┐  │
│                                  │ RemoteLookupComponent «IRemoteLookup»  │  │
│                                  │  (placeholder) receptacles: [logger]   │  │
│                                  └────────────────────────────────────────┘  │
│                                                                              │
│  ┌───────────────────────────────────────────────────────────────────────┐   │
│  │ SPDKEnvComponent                                    «ISPDKEnv»        │   │
│  │  • DPDK/EAL init  • VFIO probing  • hugepages  • BAR1 registration    │   │
│  └───────────────────────────────┬───────────────────────────────────────┘   │
│                                  │                                            │
│                                  ▼ VFIO                                      │
│              ┌───────────────────────────────────────────────┐            │
│              │  NVMe device(s) [0..N]  --device-pci           │            │
│              │  cold reads DMA directly into GPU BAR1 ring    │            │
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
| DispatchMapComponent | IDispatchMap | eviction_policy, logger |
| **DispatcherP2pComponent** | **IDispatcher** | **dispatch_map, memory_tier, gpu_services, spdk_env, logger, remote_lookup** |

The only composition difference from the `full` profile is the dispatcher row:
`full` wires `DispatcherComponent` (crate `dispatcher`), `full-p2p` wires
`DispatcherP2pComponent` (crate `dispatcher-p2p`). Receptacles, wiring, init
order, and exports are otherwise identical.

## Initialization Order

1. **LoggerComponent** — console/file logging
2. **SPDKEnvComponent** — DPDK/EAL init, VFIO device discovery
3. **GpuServicesComponent** — CUDA device init (P2P/BAR1 capable)
4. **EvictionPolicyLruComponent** — LRU eviction policy (shared by dispatch-map and memory-tier)
5. **DispatchMapComponent** — key→location table (bound to eviction_policy)
6. **MemoryTierComponent** — mmap DRAM pool, CUDA-pinned via `cudaHostRegister` (bound to eviction_policy)
7. **RemoteLookupComponent** — placeholder remote-cache integration
8. **DispatcherP2pComponent** — top-level orchestrator (bound to dispatch_map, memory_tier, gpu, spdk_env, remote_lookup)
   - Internally creates **DataDrive[0..N]**: one (BlockDeviceSpdkNvme + ExtentManager) pair per `--device-pci` address
   - Allocates the **P2pRing** — GPU BAR1 staging ring, GDRCopy-mapped and SPDK-registered (`P2P_RING_SLOTS = 64`)
   - Starts the **P2pColdReadPool** — per-drive cold-read workers with pre-connected NVMe channels + CUDA streams
   - Allocates the **PipelineRing** (DRAM-bounce cold fallback)
   - Creates dedicated **warm_stream** for async memory-tier→GPU DMA
   - Starts **ParallelBackgroundWriter** for async DRAM→SSD write-through
   - Starts **BackgroundEvictor** for SSD-tier space reclamation (threshold-based)
   - Starts **MemoryTierEvictor** for background DRAM→SSD demotion
   - Starts **DramBackfillWorker** to promote P2P-served entries back into DRAM

## Data Flow

### Populate (GPU → DRAM → SSD)
```
Client GPU ──cudaIPC──▶ Server GPU ptr ──copy_gpu_to_memory_async (D2H)──▶ Memory-tier DRAM slot
    ──ParallelBackgroundWriter (async write-through)──▶ NVMe data drive (via extent manager)
```

Entry registered in dispatch-map as `MemoryTier` immediately (available for lookup).
Background writer persists to SSD and records `ssd_offset` for durability. Identical to the `full` profile.

### Lookup — Warm Path (DRAM → GPU)
```
Memory-tier DRAM slot ──cudaMemcpyAsync H2D (warm_stream)──▶ Server GPU ptr ──cudaIPC──▶ Client GPU
```

LRU timestamp refreshed on hit. Zero-copy from pinned memory-tier pool. Identical to `full`.

### Lookup — Cold Path (SSD → GPU BAR1 ring → GPU VRAM)  *(profile-specific)*
```
NVMe data drive ──NVMe read (DMA into BAR1 slot)──▶ GPU BAR1 staging ring
    ──device-to-device copy──▶ Client GPU VRAM
    (entry stays BlockDevice; DramBackfillWorker later promotes SSD→DRAM)
```

The NVMe controller DMAs directly into GPU BAR1 memory (a single PCIe hop) and a
D2D copy delivers it into the client's destination. The dispatch-map entry is
**not** promoted during the serve; the `DramBackfillWorker` asynchronously reads
SSD→DRAM afterward and flips the entry to `MemoryTier`. Multi-region lookups are
not served over P2P — they fall back to the DRAM-bounce path (or are rejected
with `InvalidParameter` where a single region is required).

### Eviction (DRAM → SSD)
```
Memory-tier full / MemoryTierEvictor threshold ──LRU──▶ Dispatch-map entry MemoryTier → BlockDevice
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
shared `/dev/shm` mailbox path *is* the endpoint. `--ipc=host` does double duty:
the host server opens the container's CUDA IPC handles, and the container sees
the host `/dev/shm` mailbox.

## shmq Ops (opcode-framed, see `lib/shmq-dispatcher/src/wire.rs`)

The transport is a small opcode-based binary framing carried in the `/dev/shm`
mailbox. Each op maps to an `IDispatcher` method.

| Op | Request | Response | Description |
|-----|---------|----------|-------------|
| Populate | HandleBatch | per-op status | GPU→DRAM→SSD cache insertion |
| Lookup | HandleBatch | per-op status | Serve from DRAM (warm) or SSD→GPU via BAR1 ring (cold P2P) |
| Check | key list | per-key existence | Existence check (no data transfer) |
| Remove | key list | per-key status | Evict from DRAM + SSD, free extents |
| Touch | key list | per-key status | Refresh LRU timestamp without DMA |
| ClearMemoryTier | (empty) | entries_cleared | Evict all DRAM entries |
| FlushToSsd | (empty) | jobs_flushed | Force pending write-through to complete |
| GetIoStats | (empty) | I/O counters | Read/write op, byte, and latency totals |

## Notes

- The PlantUML source is in `certus-server-deployment.puml`; render with the PlantUML jar (see repo skill) to produce the SVG
- All component bindings use the COM-style `receptacle.connect(Arc<dyn Interface>)` pattern
- The Dispatcher internally instantiates and owns its DataDrive components (not externally wired)
- Memory-tier pool is registered with CUDA (`cudaHostRegister`) and SPDK (`spdk_mem_register`); the GPU BAR1 ring is separately GDRCopy-mapped and SPDK-registered so NVMe can DMA into it
- The P2P cold path serves SSD→GPU in a single PCIe hop; the `PipelineRing` remains as the DRAM-bounce fallback
- Dispatcher variant is selected via YAML profile: `full` uses `dispatcher`, `full-p2p` uses `dispatcher-p2p`
