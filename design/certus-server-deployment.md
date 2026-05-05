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
│  │  • Batch gRPC requests                   │       └─────────┬─────────┘    │
│  └──────────────────┬───────────────────────┘                 │              │
│                     │                                         │ CUDA IPC     │
└─────────────────────┼─────────────────────────────────────────┼──────────────┘
                      │ gRPC (protobuf/TCP)                     │
                      ▼                                         ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│ certus-server Process                                                        │
│                                                                              │
│  ┌────────────────────────────────────────────────────────────────────────┐  │
│  │ gRPC Service Layer (tonic)                                             │  │
│  │  • Populate / Lookup / Check / Remove RPCs                             │  │
│  │  • cudaIpcOpenMemHandle / cudaIpcCloseMemHandle                        │  │
│  │  • Batch→singular mapping, duplicate-key rejection                     │  │
│  └────────────────────────────────┬───────────────────────────────────────┘  │
│                                   │                                          │
│  ┌────────────────────────────────▼───────────────────────────────────────┐  │
│  │ DispatcherComponentV0                            «IDispatcher»         │  │
│  │                                                                        │  │
│  │  receptacles:                                                          │  │
│  │    ├─ dispatch_map ──────────────────┐                                 │  │
│  │    ├─ gpu_services ───────┐          │                                 │  │
│  │    ├─ spdk_env ───┐       │          │                                 │  │
│  │    └─ logger ─┐   │       │          │                                 │  │
│  │               │   │       │          │                                 │  │
│  │  ┌────────────┼───┼───────┼──────────┼─────────────────────────────┐   │  │
│  │  │ Inner: DataDrive[0..N] │(one per  │--data-pci address)          │   │  │
│  │  │            │   │       │          │                             │   │  │
│  │  │  ┌─────────┼───┼───────┼─┐  ┌─────┼─────────────────────────┐   │   │  │
│  │  │  │ BlockDeviceSpdkNvmeV2 │  │ ExtentManagerV2               │   │   │  │
│  │  │  │ «IBlockDevice»        │  │ «IExtentManager»              │   │   │  │
│  │  │  │ «IBlockDeviceAdmin»   │  │  receptacles:                 │   │   │  │
│  │  │  │  receptacles:         │  │    ├─ metadata_device ──▶[BD] │   │   │  │
│  │  │  │    ├─ spdk_env        │  │    └─ logger                  │   │   │  │
│  │  │  │    └─ logger          │  └───────────────────────────────┘   │   │  │
│  │  │  └───────────────────────┘                                      │   │  │
│  │  └─────────────────────────────────────────────────────────────────┘   │  │
│  │               │   │       │          │                                 │  │
│  │  [BackgroundWriter] ──────┼──────────┼── WriteJob ──▶ DataDrive        │  │
│  └───────────────┼───┼───────┼──────────┼─────────────────────────────────┘  │
│                  │   │       │          │                                    │
│                  ▼   │       ▼          ▼                                    │
│  ┌────────────────┐  │  ┌─────────────────────────────────────────────────┐  │
│  │ LoggerCompV1   │  │  │ DispatchMapComponentV0           «IDispatchMap» │  │
│  │ «ILogger»      │  │  │                                                 │  │
│  │                │  │  │  receptacles:                                   │  │
│  │ (all comps     │  │  │    ├─ extent_manager ──┐                        │  │
│  │  bind here)    │  │  │    └─ logger           │                        │  │
│  └────────────────┘  │  │                        │                        │  │
│                      │  │  [Entry Table]         │                        │  │
│                      │  │   key → {Staging|      │                        │  │
│                      │  │          BlockDevice}  │                        │  │
│                      │  └────────────────────────┼────────────────────────┘  │
│                      │                           │                           │
│                      │                           ▼                           │
│                      │  ┌─────────────────────────────────────────────────┐  │
│                      │  │ Metadata ExtentManagerV2       «IExtentManager» │  │
│                      │  │  receptacles:                                   │  │
│                      │  │    ├─ metadata_device ─┐                        │  │
│                      │  │    └─ logger           │                        │  │
│                      │  └────────────────────────┼────────────────────────┘  │
│                      │                           │                           │
│                      │                           ▼                           │
│                      │  ┌─────────────────────────────────────────────────┐  │
│                      │  │ Metadata BlockDeviceSpdkNvmeV2                  │  │
│                      │  │ «IBlockDevice, IBlockDeviceAdmin»               │  │
│                      │  │  receptacles:                                   │  │
│                      │  │    ├─ spdk_env ────┐                            │  │
│                      │  │    └─ logger       │                            │  │
│                      │  └────────────────────┼────────────────────────────┘  │
│                      │                       │                               │
│                      ▼                       ▼                               │
│  ┌───────────────────────────────────────────────────────────┐               │
│  │ SPDKEnvComponent                          «ISPDKEnv»      │               │
│  │  • DPDK/EAL initialization                                │               │
│  │  • VFIO device discovery & probing                        │               │
│  │  • Hugepage memory management                             │               │
│  └───────────────────────┬───────────────────────────────────┘               │
│                          │                                                   │
│  ┌───────────────────────────────────────────────────────────┐               │
│  │ GpuServicesComponentV0                    «IGpuServices»  │               │
│  │  receptacles:                                             │               │
│  │    └─ logger                                              │               │
│  │  • cudaMemcpy (H2D / D2H)                                 │─▶ GPU Memory  │
│  │  • DMA buffer management                                  │  (server ctx) │
│  └───────────────────────────────────────────────────────────┘               │
│                          │                                                   │
│                          ▼ VFIO                                              │
│              ┌───────────────────────┐  ┌───────────────────────┐            │
│              │  NVMe (metadata)      │  │  NVMe (data) [0..N]   │            │
│              │  --metadata-pci       │  │  --data-pci           │            │
│              └───────────────────────┘  └───────────────────────┘            │
└──────────────────────────────────────────────────────────────────────────────┘
```

## Component Summary

| Component | Version | Provides | Receptacles |
|-----------|---------|----------|-------------|
| SPDKEnvComponent | 0.1.0 | ISPDKEnv | — |
| LoggerComponentV1 | — | ILogger | — |
| GpuServicesComponentV0 | 0.1.0 | IGpuServices | logger |
| BlockDeviceSpdkNvmeV2 | 0.2.0 | IBlockDevice, IBlockDeviceAdmin | spdk_env, logger |
| ExtentManagerV2 | 0.3.0 | IExtentManager | metadata_device, logger |
| DispatchMapComponentV0 | 0.1.0 | IDispatchMap | extent_manager, logger |
| DispatcherComponentV0 | 0.1.0 | IDispatcher | dispatch_map, gpu_services, spdk_env, logger |

## Initialization Order

1. **SPDKEnvComponent** — DPDK/EAL init, VFIO device discovery
2. **LoggerComponentV1** — console/file logging
3. **GpuServicesComponentV0** — CUDA device init
4. **Metadata BlockDeviceSpdkNvmeV2** — NVMe controller for metadata (bound to spdk_env)
5. **Metadata ExtentManagerV2** — block allocator over metadata device
6. **DispatchMapComponentV0** — key→location table (bound to metadata extent manager)
7. **DispatcherComponentV0** — top-level orchestrator (bound to dispatch_map, gpu, spdk_env)
   - Internally creates **DataDrive[0..N]**: one (BlockDeviceSpdkNvmeV2 + ExtentManagerV2) pair per `--data-pci` address
   - Starts **BackgroundWriter** thread for async staging→storage conversion

## Data Flow

### Populate (GPU → NVMe)
```
Client GPU ──cudaIPC──▶ Server GPU ptr ──cudaMemcpy D2H──▶ DMA staging buffer
    ──BackgroundWriter──▶ NVMe data drive (via extent manager)
```

### Lookup (NVMe → GPU)
```
NVMe data drive ──SPDK read──▶ DMA staging buffer ──cudaMemcpy H2D──▶ Server GPU ptr
    ──cudaIPC──▶ Client GPU
```

## Notes

- The PlantUML source is in `certus-server-deployment.puml` for rendered diagrams
- To render: `plantuml certus-server-deployment.puml` (produces PNG/SVG)
- All component bindings use the COM-style `receptacle.connect(Arc<dyn Interface>)` pattern
- The Dispatcher internally instantiates and owns its DataDrive components (not externally wired)
