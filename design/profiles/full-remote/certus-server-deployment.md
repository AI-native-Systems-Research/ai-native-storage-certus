# Certus Server — Component Deployment Diagram (`full-remote` Profile)

## Component Topology

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ Client Process                                                               │
│                                                                              │
│  ┌──────────────────────────────────────────┐                                │
│  │ Python Test Client                       │       ┌───────────────────┐    │
│  │  • PyTorch GPU allocation                │──────▶│ GPU Memory        │    │
│  │  • cudaIpcGetMemHandle (64-byte handle)  │       │ (client context)  │    │
│  │  • Batch gRPC requests                   │       └─────────┬─────────┘    │
│  └──────────────────┬───────────────────────┘                 │              │
│                     │                                         │ CUDA IPC     │
└─────────────────────┼─────────────────────────────────────────┼──────────────┘
                      │ gRPC (protobuf/TCP, optional TLS)       │
                      ▼                                         ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│ certus-server Process (full-remote profile)                                  │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ gRPC Service Layer (tonic)                                             │  │
│  │  • Populate / Lookup / Check / Remove / Touch / ClearMemoryTier RPCs   │  │
│  │  • cudaIpcOpenMemHandle / cudaIpcCloseMemHandle (cached per batch)     │  │
│  │  • Batch→singular mapping, duplicate-key rejection                     │  │
│  └────────────────────────────────┬───────────────────────────────────────┘  │
│                                   │                                          │
│  ┌────────────────────────────────▼───────────────────────────────────────┐  │
│  │ DispatcherComponent                              «IDispatcher»         │  │
│  │                                                                        │  │
│  │  receptacles:                                                          │  │
│  │    ├─ dispatch_map ──────────────────┐                                 │  │
│  │    ├─ memory_tier ────────┐          │                                 │  │
│  │    ├─ gpu_services ───────┼──┐       │                                 │  │
│  │    ├─ spdk_env ───┐       │  │       │                                 │  │
│  │    ├─ remote_lookup ──────┼──┼──┐    │                                 │  │
│  │    └─ logger ─┐   │       │  │  │    │                                 │  │
│  │               │   │       │  │  │    │                                 │  │
│  │  ┌────────────┼───┼───────┼──┼──┼────┼─────────────────────────────┐   │  │
│  │  │ Inner: DataDrive[0..N] │  │  │    │(one per --device-pci)       │   │  │
│  │  │  ┌─────────────────────┼──┼──┼─┐  ┌───────────────────────────┐│   │  │
│  │  │  │ BlockDeviceSpdkNvme   │ │  │ ExtentManager             ││   │  │
│  │  │  │ «IBlockDevice»         │ │  │ «IExtentManager»          ││   │  │
│  │  │  │ «IBlockDeviceAdmin»    │ │  │  receptacles:             ││   │  │
│  │  │  │  receptacles:          │ │  │    ├─ metadata_device     ││   │  │
│  │  │  │    ├─ spdk_env         │ │  │    └─ logger              ││   │  │
│  │  │  │    └─ logger           │ │  └───────────────────────────┘│   │  │
│  │  │  └────────────────────────┘ │                               │   │  │
│  │  └───────────────────────────┼──┼─┼───────────────────────────────┘   │  │
│  │               │   │       │  │  │ │                                    │  │
│  │  [ParallelBackgroundWriter]──┼──┼─┼── WriteJob ──▶ DataDrive           │  │
│  │  [BackgroundEvictor] ────────┼──┼─┼── Evict stale SSD extents          │  │
│  │  [PipelineRing] ────────────┘  │ │── Pipelined SSD→DRAM→GPU reads     │  │
│  └───────────────┼───┼────────────┼─┼────────────────────────────────────┘  │
│                  │   │       │    │ │                                        │
│                  ▼   │       │    ▼ │                                        │
│  ┌────────────────┐  │  ┌─────────┐│  ┌──────────────────────────────────┐  │
│  │ LoggerComponent│  │  │MemoryTier││  │ DispatchMapComponent             │  │
│  │ «ILogger»     │  │  │Component ││  │ «IDispatchMap»                   │  │
│  │               │  │  │«IMemory  ││  │  receptacles:                    │  │
│  │               │  │  │ Tier»    ││  │    ├─ eviction_policy            │  │
│  │               │  │  │  recepts:││  │    └─ logger                     │  │
│  │               │  │  │   ├evict ││  └──────────────────────────────────┘  │
│  │               │  │  │   └logger││                                        │
│  └────────────────┘  │  └─────────┘│                                        │
│                      │             │                                        │
│                      ▼             ▼                                        │
│  ┌───────────────────────────┐  ┌────────────────────────────────────────┐  │
│  │ GpuServicesComponent      │  │ EvictionPolicyLruComponent             │  │
│  │ «IGpuServices»            │  │ «IEvictionPolicy»                      │  │
│  │  receptacles: [logger]    │  │  receptacles: [logger]                 │  │
│  └───────────────────────────┘  └────────────────────────────────────────┘  │
│                                                                              │
│  ┌───────────────────────────────────────────────────────────────────────┐   │
│  │ SPDKEnvComponent                                    «ISPDKEnv»        │   │
│  └───────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
│  ═══════════════════════════════════════════════════════════════════════════  │
│  REMOTE CACHE LAYER (unique to full-remote)                                  │
│  ═══════════════════════════════════════════════════════════════════════════  │
│                                                                              │
│  ┌───────────────────────────────────────────────────────────────────────┐   │
│  │ RemoteLookupComponent                           «IRemoteLookup»       │   │
│  │  receptacles: [logger]                                                │   │
│  │  • Forwards local cache misses to peer nodes                          │   │
│  │  • batch_lookup / join_cluster / leave_cluster                        │   │
│  └─────────────────────────────────┬─────────────────────────────────────┘   │
│                                    │ RDMA (outgoing)                         │
│  ┌─────────────────────────────────┼─────────────────────────────────────┐   │
│  │ RemoteLookupRdmaInitiatorComponent   │          «IRemoteLookupRdmaInitiator»    │   │
│  │  receptacles: [dispatcher, logger]                                    │   │
│  │  • Accepts incoming RDMA requests from peer nodes                     │   │
│  │  • Resolves keys via local dispatcher                                 │   │
│  │  • Returns zero-copy LookupRef for RDMA Write                         │   │
│  │  • handle_lookup / handle_check / handle_batch_lookup / release_lookup│   │
│  └─────────────────────────────────┼─────────────────────────────────────┘   │
│                                    │ RDMA (incoming)                         │
└────────────────────────────────────┼─────────────────────────────────────────┘
                                     ▼
                         ┌───────────────────────┐
                         │  Peer Certus Nodes    │
                         │  (full-remote)        │
                         └───────────────────────┘
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
| DispatcherComponent | IDispatcher | dispatch_map, memory_tier, gpu_services, spdk_env, logger, remote_lookup |
| **RemoteLookupRdmaInitiatorComponent** | **IRemoteLookupRdmaInitiator** | **dispatcher, logger** |

## Initialization Order

1. **LoggerComponent** — console/file logging
2. **SPDKEnvComponent** — DPDK/EAL init, VFIO device discovery
3. **GpuServicesComponent** — CUDA device init
4. **EvictionPolicyLruComponent** — shared LRU policy
5. **DispatchMapComponent** — key→location table
6. **MemoryTierComponent** — mmap DRAM pool, CUDA-pinned via `cudaHostRegister`
7. **RemoteLookupComponent** — cluster client, joins peer network
8. **DispatcherComponent** — top-level orchestrator
   - Internally creates **DataDrive[0..N]**: one (BlockDeviceSpdkNvme + ExtentManager) per `--device-pci`
   - Allocates **PipelineRing** for pipelined cold reads
   - Creates **warm_stream** for async memory-tier→GPU DMA
   - Starts **ParallelBackgroundWriter** for async write-through
   - Starts **BackgroundEvictor** for SSD space reclamation
9. **RemoteLookupRdmaInitiatorComponent** — RDMA listener, resolves incoming requests via dispatcher

## Data Flow

### Populate (GPU → DRAM → SSD)
```
Client GPU ──cudaIPC──▶ Server GPU ptr ──cudaMemcpy D2H──▶ Memory-tier DRAM slot
    ──ParallelBackgroundWriter (async write-through)──▶ NVMe data drive
```

Entry registered in dispatch-map as `MemoryTier` immediately. Background writer persists to SSD. **No remote propagation** — populate is local-only.

### Lookup — Warm Path (DRAM → GPU)
```
Memory-tier DRAM slot ──cudaMemcpyAsync H2D (warm_stream)──▶ Client GPU
```

### Lookup — Cold Path (Local SSD → DRAM → GPU)
```
NVMe data drive ──PipelineRing (async chunked reads)──▶ Memory-tier DRAM slot
    ──cudaMemcpyAsync H2D──▶ Client GPU
```

### Lookup — Remote Path (Peer Node → This Node → GPU)
```
Local miss ──RemoteLookup.batch_lookup──▶ Peer node (via RDMA)
    ──RDMA Write──▶ Local memory ──(optional promote)──▶ Memory-tier
    ──cudaMemcpyAsync H2D──▶ Client GPU
```

### Incoming Remote Request (Peer → This Node)
```
Peer RDMA request ──RemoteLookupRdmaInitiator.handle_lookup──▶ Dispatcher.lookup
    ──LookupRef (pinned memory-tier pointer)──▶ RDMA Write to peer
    ──release_lookup (unpin)
```

### Eviction
```
DRAM full ──LRU evict──▶ MemoryTier → BlockDevice (data on SSD via write-through)
SSD > threshold ──BackgroundEvictor──▶ Remove oldest extents
```

## CLI Options

```
certus-server-yaml \
    --device-pci DDDD:BB:DD.F [--device-pci ...] \
    --memory-tier-size 4G \
    --format \
    --listen 0.0.0.0:50051
```

| Flag | Description |
|------|-------------|
| `--device-pci` | PCI address(es) of NVMe device(s), repeatable |
| `--device-path` | Filesystem device path (alternative to PCI, e.g., `/dev/null` for testing) |
| `--memory-tier-size` | DRAM pool size (e.g., `256M`, `1G`, `4G`) |
| `--format` | Format SSD extents on startup |
| `--listen` | gRPC bind address (default `0.0.0.0:50051`) |
| `--tls-cert` / `--tls-key` | Enable TLS for gRPC transport |

## gRPC API (certus.dispatcher.v1)

| RPC | Request | Response | Description |
|-----|---------|----------|-------------|
| Populate | BatchPopulateRequest | BatchPopulateResponse | GPU→DRAM→SSD cache insertion |
| Lookup | BatchLookupRequest | BatchLookupResponse | Serve from DRAM, SSD, or remote peer |
| Check | BatchCheckRequest | BatchCheckResponse | Existence check (no data transfer) |
| Remove | BatchRemoveRequest | BatchRemoveResponse | Evict from DRAM + SSD, free extents |
| Touch | BatchTouchRequest | BatchTouchResponse | Refresh LRU timestamp without DMA |
| ClearMemoryTier | ClearMemoryTierRequest | ClearMemoryTierResponse | Evict all DRAM entries |
| FlushToSsd | FlushToSsdRequest | FlushToSsdResponse | Force pending write-through to complete |

## Notes

- RemoteLookupRdmaInitiator is initialized last because it needs a fully-wired dispatcher
- All component bindings use the COM-style `receptacle.connect(Arc<dyn Interface>)` pattern
- The remote_lookup receptacle on the dispatcher enables miss-forwarding to peers
- LookupRef holds a dispatch-map read reference — failure to release blocks eviction of that entry
- Memory-tier pool is registered with CUDA (`cudaHostRegister`) and SPDK (`spdk_mem_register`) for zero-copy
- Cluster membership managed via `join_cluster`/`leave_cluster` on RemoteLookupComponent
