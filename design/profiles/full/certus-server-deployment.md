# Certus Server — Component Deployment Diagram

## Component Topology

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ Client Process (vLLM + certus-shmq-connector)                                │
│   • cudaIpcGetMemHandle per layer tensor (64-byte handles)                   │
│   • Batched shmq requests (Reserve/CopyToStore/CommitStore, Lookup, ...)     │
│                                                    GPU Memory (client ctx)   │
└───────────────┬────────────────────────────────────────────┬─────────────────┘
                │ shmq (/dev/shm mailbox, shared IPC ns)     │ CUDA IPC
                ▼                                            ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│ certus-server-yaml Process (CERTUS_PROFILE=full)                             │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ shmq-dispatcher (translate + serve)                                    │  │
│  │  • poller + N workers (--channels), shmq-reaper                        │  │
│  │  • IPC-handle cache (open/close, grace-period reap)                    │  │
│  │  • shared reserve-batch backpressure deadline                          │  │
│  └───────────────────────────────────┬────────────────────────────────────┘  │
│                                      ▼                                       │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ DispatcherComponent                                   «IDispatcher»    │  │
│  │  receptacles: dispatch_map, memory_tier, gpu_services, spdk_env,       │  │
│  │               remote_lookup, logger                                    │  │
│  │                                                                        │  │
│  │  ┌──────────────────────────────────────────────────────────────────┐  │  │
│  │  │ DataDrive 0..N  (one per --device-pci; created by factories)     │  │  │
│  │  │   BlockDeviceSpdkNvme «IBlockDevice, IBlockDeviceAdmin»          │  │  │
│  │  │     receptacles: spdk_env, logger                                │  │  │
│  │  │   DiskPartitionManager → GPT [metadata | ext-metadata | data]    │  │  │
│  │  │   ExtentManager «IExtentManager» (metadata_device → BlockDevice) │  │  │
│  │  │   ChannelPool (pre-connected NVMe client channels)               │  │  │
│  │  └──────────────────────────────────────────────────────────────────┘  │  │
│  │  ParallelBackgroundWriter   BackgroundEvictor   [MemoryTierEvictor]    │  │
│  │  PipelineRing (StagingPool)  DeviceStreams{warm,store,pipe[2]}/GPU     │  │
│  │  EvictionSender → TakeEvents   TierEventCounters                       │  │
│  └──────┬──────────────┬──────────────┬──────────────┬─────────────┬──────┘  │
│         ▼              ▼              ▼              ▼             ▼         │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐  │
│  │DispatchMap │ │MemoryTier  │ │GpuServices │ │SPDKEnv     │ │RemoteLookup│  │
│  │«IDispatch  │ │«IMemory    │ │«IGpu       │ │«ISPDKEnv»  │ │«IRemote    │  │
│  │  Map»      │ │  Tier»     │ │  Services» │ │            │ │  Lookup»   │  │
│  │key→{MT|BD} │ │mmap pool,  │ │CUDA DMA,   │ │EAL, VFIO,  │ │(no init    │  │
│  │refcounts   │ │first-fit   │ │streams,    │ │hugepages   │ │ hook: all  │  │
│  │            │ │4 KiB align │ │pinned bufs │ │            │ │ NotFound)  │  │
│  └─────┬──────┘ └─────┬──────┘ └────────────┘ └────────────┘ └────────────┘  │
│        └──────┬───────┘                                                      │
│               ▼                                                              │
│  ┌──────────────────────────────┐   ┌──────────────────────────────┐         │
│  │ EvictionPolicyLruComponent   │   │ LoggerComponent «ILogger»    │         │
│  │ «IEvictionPolicy»            │   │ (bound by every component)   │         │
│  │ O(1) index-linked LRU        │   └──────────────────────────────┘         │
│  │ one pool per consumer        │                                            │
│  └──────────────────────────────┘                                            │
│                                                                              │
│        GPU Memory (server ctx) ◀── DMA ── GpuServices                        │
└──────────────────────────────────────┬───────────────────────────────────────┘
                                       ▼ VFIO (SPDK userspace NVMe)
                   ┌────────────────────────────────────────────┐
                   │ NVMe device(s) 0..N — metadata + data      │
                   │ co-located per drive (GPT partitions)      │
                   └────────────────────────────────────────────┘
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
| DispatchMapComponent | IDispatchMap | extent_manager (not bound in this profile), eviction_policy, logger |
| DispatcherComponent | IDispatcher | dispatch_map, memory_tier, gpu_services, spdk_env, logger, remote_lookup |

## Initialization Order

1. **LoggerComponent** — console/file logging
2. **SPDKEnvComponent** — DPDK/EAL init, VFIO device discovery (`--device-pci` or first `--drive-count` drives); restores the caller's CPU affinity afterwards
3. **GpuServicesComponent** — CUDA device init
4. **EvictionPolicyLruComponent** — LRU eviction policy (shared by dispatch-map and memory-tier)
5. **DispatchMapComponent** — key→location table (bound to eviction_policy)
6. **MemoryTierComponent** — mmap DRAM pool, CUDA-pinned via `cudaHostRegister` (bound to eviction_policy)
7. **RemoteLookupComponent** — wired to the dispatcher but **not initialized** in this profile (no `init_hook`); `batch_lookup` returns `NotFound` for every key
8. **DispatcherComponent** — top-level orchestrator (bound to dispatch_map, memory_tier, gpu, spdk_env, remote_lookup)
   - Internally creates **DataDrive 0..N**: one (BlockDeviceSpdkNvme + ExtentManager) pair per `--device-pci` address. Each drive's NVMe poller is pinned to `--poller-base-cpu + i`, or NUMA-locally (skipping the node's first two cores) when unset
   - Lays out a GPT on each drive via **DiskPartitionManager** — metadata (128 MiB), extended metadata (128 MiB, reserved), data (rest) — and sets the extent manager's metadata/data base LBAs; `--format` reformats, otherwise an incompatible table is an error
   - Rebuilds the dispatch-map from each drive's extents (`for_each_extent` → `recover_extent`)
   - Allocates **PipelineRing**: chunk size = drive MDTS, plus the cold-load **StagingPool** (64 × 16 MiB CUDA-pinned + SPDK-registered buffers by default)
   - Creates dedicated **warm_stream** (CUDA stream) for async memory-tier→GPU DMA
   - Starts **ParallelBackgroundWriter** for async DRAM→SSD write-through
   - Starts **BackgroundEvictor** thread for SSD-tier space reclamation (threshold-based)
   - Starts **MemoryTierEvictor** thread for DRAM→SSD demotion under memory-tier pressure — only when `--memory-tier-eviction-threshold` > 0 (off by default)
9. **shmq serve** — mailbox at `--shm-path`, poller + `--channels` workers + `shmq-reaper`; thread CPU bindings are logged at startup

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
NVMe data drives 0..N ──scatter-gather ReadAsync (zero-copy)──▶ Memory-tier DRAM slots
    ──cudaMemcpyAsync H2D (pipe streams)──▶ Server GPU ptr ──cudaIPC──▶ Client GPU
```

All drives are driven from one multiplexed poll loop on the request's worker thread.
Each entry is promoted back to memory-tier in place (`promote_block_to_memory_tier`).
Evicts LRU entries if DRAM pool is full before insertion; if nothing is evictable,
serves the block uncached through a StagingPool buffer (SSD → staging → GPU).

### Eviction (DRAM → SSD only)
```
Memory-tier full ──LRU evict──▶ Dispatch-map entry transitions from MemoryTier → BlockDevice
    (data already on SSD via write-through; only pointer/state change)
    ──EvictionEvent{Demoted}──▶ TakeEvents (client)
```

Pinned or not-yet-persisted entries are skipped, never removed. If nothing can be
freed, `reserve_memory` backpressures (20 ms retries, up to `--store-backpressure-ms`)
and then the store is refused or dropped.

### SSD Eviction (BackgroundEvictor)
```
SSD usage > threshold ──oldest_keys scan──▶ Remove extents from SSD
    ──Frees blocks in extent manager──▶ Space available for new writes
```

## CLI Options

```
CERTUS_PROFILE=full cargo build -p certus-server-yaml   # profile selected at build time
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
| `--drive-count` | Use the first N discovered NVMe drives (alternative to `--device-pci`) |
| `--memory-tier-size` | DRAM pool size (e.g., `256M`, `1G`, `4G`; default 2G) |
| `--format` | Format SSD partitions/extents on startup (start fresh) |
| `--shm-path` | Path to the shared-memory mailbox file (default `/dev/shm/certus-shmq`) |
| `--channels` | Number of mailbox channels (= max in-flight requests = worker threads; default 8) |
| `--cap-req` / `--cap-resp` | Per-channel request / response capacity (default 1M / 128K) |
| `--reserve-timeout-secs` | Reclaim reservations never committed/aborted (default 30) |
| `--shmq-poller-cpu` / `--shmq-poller-stats` | Pin the shmq poller; log poller fairness stats |
| `--poller-base-cpu` | Pin NVMe poller for drive i to core base+i (default: NUMA-local auto) |
| `--max-eviction-attempts` | Eviction attempts before pool-full (default 2048) |
| `--store-backpressure-ms` | Store retry budget on a full tier (default 5000; 0 = fail fast) |
| `--memory-tier-eviction-threshold` | Enable background DRAM→SSD demotion at this utilization (default 0.0 = off) |
| `--metrics-port` / `--otel-endpoint` / `--otel-service-name` | Prometheus / OTLP export (OTLP needs `--features otel`) |
| `--backfill-delay-ms`, `--rl-group` | Accepted but have no effect in `full` (p2p-only; remote-lookup uninitialized) |

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
| Lookup | HandleBatch | per-key tier byte (0 miss/err, 1 DRAM, 2 SSD, 3 remote) | Serve from DRAM or promote from SSD→GPU |
| Check | key list | per-key existence | Existence check (no data transfer) |
| Remove | key list | per-key status | Evict from DRAM + SSD, free extents |
| Touch | key list | per-key status | Refresh LRU timestamp without DMA |
| TouchCheck | key list | per-key state | Touch + Check fused in one pass |
| Reserve / CopyToStore / CommitStore / AbortStore | keys + sizes / HandleBatch / keys | per-op status | Split store: `reserve_memory` (shared batch deadline) → `copy_gpu_to_memory_async` → `copy_gpu_to_memory_completed` / `release_memory` |
| Pin / Unpin | key list | per-key status | Eviction-protection read refs |
| TakeEvents | max | events + dropped count | Drain `EvictionEvent`s (Demoted / Removed) |
| ClearMemoryTier | (empty) | entries_cleared | Evict all DRAM entries |
| FlushToSsd | (empty) | jobs_flushed | Force pending write-through to complete |
| GetIoStats | (empty) | I/O counters | Read/write op, byte, and latency totals |

## Notes

- The PlantUML source is in `certus-server-deployment.puml` for rendered diagrams
- To render: `plantuml certus-server-deployment.puml` (produces PNG/SVG)
- All component bindings use the COM-style `receptacle.connect(Arc<dyn Interface>)` pattern
- The Dispatcher internally instantiates and owns its DataDrive components (not externally wired)
- Memory-tier pool is registered with CUDA (`cudaHostRegister`) for pinned zero-copy DMA
- PipelineRing's StagingPool pre-allocates `cold_staging_slots` (64) CUDA-pinned SPDK-registered buffers of 16 MiB, used only when the memory tier is saturated
- Dispatcher variant is selected via YAML profile at build time (`CERTUS_PROFILE`, e.g., `full`, `full-p2p`, `full-remote`)
- The `full-optimized` profile is identical except for the eviction policy (`eviction-policy-optimized`)
