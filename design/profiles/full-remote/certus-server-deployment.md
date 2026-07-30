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
│  │  receptacles: [zyre, dispatch_map, memory_tier, dispatcher,           │   │
│  │                initiator, responder, responder_admin, logger]         │   │
│  │  • Orchestrator: reserve landing slot, query peers, submit push       │   │
│  │  • initialize / batch_lookup([(key,size)]) / join_cluster / leave     │   │
│  └───────┬───────────────────┬────────────────────┬─────────────────────┘   │
│          │ drives            │ reserves            │ discovers/queries       │
│  ┌───────▼──────────┐ ┌──────▼───────────┐ ┌───────▼──────────────────────┐   │
│  │ RdmaInitiator    │ │ RdmaResponder    │ │ ZyreComponent    «IZyre»     │   │
│  │ «...RdmaInitiator»│ │ «...RdmaResponder»│ │  receptacles: []            │   │
│  │  recepts:        │ │  «...ResponderAdmin»│ │  • gossip/beacon discovery │   │
│  │  [logger,        │ │  recepts:         │ │  • create IZyreNode handle  │   │
│  │   memory_tier]   │ │  [logger,         │ └──────────────┬──────────────┘   │
│  │  • async push to │ │   memory_tier]    │                │ gossip           │
│  │    peer memory   │ │  • register pool, │                │ (KeyQuery /      │
│  │    (data-holder) │ │    accept writes  │                │  RdmaRequest)    │
│  └───────┬──────────┘ │    IN (requester) │                │                  │
│          │ RDMA OUT   └──────▲───────────┘                 │                  │
└──────────┼───────────────────┼──────────────────────────────┼──────────────────┘
           │                   │ RDMA IN                       │
           ▼                   │                               ▼
         ┌─────────────────────┴───────────────────────────────────┐
         │  Peer Certus Nodes (full-remote)                        │
         └─────────────────────────────────────────────────────────┘
```

## Component Summary

| Component | Provides | Receptacles |
|-----------|----------|-------------|
| SPDKEnvComponent | ISPDKEnv | — |
| LoggerComponent | ILogger | — |
| GpuServicesComponent | IGpuServices | logger |
| EvictionPolicyLruComponent | IEvictionPolicy | logger |
| MemoryTierComponent | IMemoryTier | logger, eviction_policy |
| BlockDeviceSpdkNvme | IBlockDevice, IBlockDeviceAdmin | spdk_env, logger |
| ExtentManager | IExtentManager | metadata_device, logger |
| DispatchMapComponent | IDispatchMap | eviction_policy, logger |
| DispatcherComponent | IDispatcher | dispatch_map, memory_tier, gpu_services, spdk_env, logger, remote_lookup |
| **ZyreComponent** | **IZyre** | **—** |
| **RemoteLookupRdmaInitiatorComponent** | **IRemoteLookupRdmaInitiator** | **logger, memory_tier** |
| **RemoteLookupRdmaResponderComponent** | **IRemoteLookupRdmaResponder, IRemoteLookupRdmaResponderAdmin** | **logger, memory_tier** |
| **RemoteLookupComponent** | **IRemoteLookup** | **zyre, dispatch_map, memory_tier, dispatcher, initiator, responder, responder_admin, logger** |

## Initialization Order

1. **LoggerComponent** — console/file logging
2. **SPDKEnvComponent** — DPDK/EAL init, VFIO device discovery
3. **GpuServicesComponent** — CUDA device init
4. **EvictionPolicyLruComponent** — shared LRU policy
5. **DispatchMapComponent** — key→location table
6. **MemoryTierComponent** — mmap DRAM pool, CUDA-pinned via `cudaHostRegister`
7. **ZyreComponent** — peer-discovery factory
8. **RemoteLookupRdmaInitiatorComponent** — outbound push side (bound to logger, memory_tier); its per-peer connection threads are spawned lazily on first use, not at initialization
9. **RemoteLookupRdmaResponderComponent** — passive accept side (bound to logger, memory_tier)
10. **RemoteLookupComponent** — orchestrator; `init_hook` brings up the responder, advertises the local RDMA endpoint via Zyre, spawns the initiator worker, and joins the discovery group
11. **DispatcherComponent** — top-level orchestrator
    - Internally creates **DataDrive[0..N]**: one (BlockDeviceSpdkNvme + ExtentManager) per `--device-pci`
    - Allocates **PipelineRing** for pipelined cold reads (8-deep, 2 CUDA streams)
    - Creates **warm_stream** for async memory-tier→GPU DMA
    - Starts **ParallelBackgroundWriter** for async write-through
    - Starts **BackgroundEvictor** for SSD space reclamation

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

### Lookup — Remote Path (this node is the requester)
```
Local miss ──RemoteLookup.batch_lookup([(key,size)])──▶ reserve landing slot
    ──SHOUT KeyQuery via Zyre──▶ Peer node
    ──WHISPER RdmaRequest(endpoint,rkey,slot)──▶ holding peer
    ──peer RDMA-writes value IN via this node's Responder──▶ Local memory
    ──(optional promote)──▶ Memory-tier ──cudaMemcpyAsync H2D──▶ Client GPU
```

### Incoming Remote Request (this node is the data-holder)
```
Peer KeyQuery/RdmaRequest ──handle_rdma_request ──▶ InitiatorCmd::Serve
    ──resolve value in Memory-tier, pin it──▶ RdmaInitiator.push_async (returns at once)
    ──[peer's connection thread] RDMA Write value OUT into peer's advertised slot
    ──completion callback: release pins──▶ PushComplete ──▶ WHISPER RdmaStatus
```

Submission returns as soon as the batch is queued; the per-peer connection thread posts
and reaps, then runs the completion callback. The read pins are owned by that callback
because the NIC keeps reading the pinned buffers after submission returns.

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

- RemoteLookup initializes before the dispatcher; `dispatcher.remote_lookup` and `remote_lookup.dispatcher` form a deliberate `Arc` cycle, severed at teardown
- All component bindings use the COM-style `receptacle.connect(Arc<dyn Interface>)` pattern
- The remote_lookup receptacle on the dispatcher enables miss-forwarding to peers
- Remote transfer is one-sided RDMA WRITE performed by the data-holder's Initiator; the requester's Responder registers the landing memory and never touches the data
- The Initiator's `push_async` enqueues and returns; each peer endpoint has one connection thread that owns its queue pair, posts, reaps, and invokes the batch's completion callback. Send-queue credits (`PUSH_WINDOW` = 128) let successive batches pipeline, and a full submit queue is rejected rather than queued so that held read pins cannot accumulate behind an unresponsive peer
- Memory-tier pool is registered with CUDA (`cudaHostRegister`) and SPDK (`spdk_mem_register`) for zero-copy, and registered `REMOTE_WRITE` by the Responder for inbound RDMA
- Peer discovery is automatic via Zyre (UDP beacon or ZeroMQ gossip); `join_cluster`/`leave_cluster` operate on named Zyre groups and are supplementary
