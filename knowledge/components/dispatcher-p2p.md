# dispatcher-p2p

**Crate**: `dispatcher-p2p`
**Path**: `components/dispatcher-p2p/`
**Version**: 0.1.0

## Description

GPUDirect P2P variant of the Certus data dispatcher. Routes cache lookups from GPU clients to either DRAM (hot path) or SSD (cold path), with the cold path using GPU BAR1 memory as DMA targets to eliminate the host DRAM bounce buffer. Provides identical `IDispatcher` interface as the standard dispatcher.

Data flow:
- **Hot path**: DRAM memory-tier → cudaMemcpyAsync H2D → GPU VRAM
- **Cold path (P2P)**: NVMe SSD → DMA → GPU BAR1 ring → D2D copy → GPU VRAM
- **Cold path (fallback)**: NVMe SSD → DMA → DRAM → H2D → GPU VRAM

## Component Definition

```
DispatcherP2pComponent {
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
}
```

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Structured logging |
| `dispatch_map` | `IDispatchMap` | Yes | Key→location index (DRAM vs SSD) with refcounted locks |
| `gpu_services` | `IGpuServices` | Yes | CUDA operations: streams, memcpy, DMA buffers, host memory registration |
| `spdk_env` | `ISPDKEnv` | Yes | SPDK environment: device enumeration, NUMA node info, hugepage DMA |
| `memory_tier` | `IMemoryTier` | Yes | Pooled DRAM cache with LRU eviction |
| `remote_lookup` | `IRemoteLookup` | No | Cross-node cache lookup for cluster misses |

## Key Semantics

- **P2P ring**: 64 pre-allocated GPU BAR1 staging slots (GDRCopy BAR1 mapping + SPDK DMA registration), partitioned lock-free across worker threads. Eliminates host DRAM as intermediary for cold reads.
- **Pipeline strategies**: single-object P2P, multi-object P2P with interleaved NVMe reads, zero-copy DRAM fallback, DRAM-only promotion.
- **Cold-read pool**: persistent worker threads with pre-connected NVMe channels.
- **Background workers**: per-drive write-through, DRAM backfill after P2P reads, SSD evictor, memory-tier evictor with exponential aggressiveness.
- **NUMA-aware CPU pinning**: auto-assigns NVMe poller threads to same-NUMA CPUs.
- **Disk partition management**: GPT with metadata, extended-metadata, and data partitions per drive.
- **Pin-safe eviction**: never frees DRAM slot while in-flight load references it.
