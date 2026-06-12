# Data Model: GPUDirect Storage Cold Path

## Entities

### P2pRing

Pre-allocated ring of GPU-resident staging buffers for NVMe-to-GPU P2P transfers.

| Field | Type | Description |
|-------|------|-------------|
| slots | Vec\<DmaBuffer\> | 64 GPU staging buffers (cudaMalloc + BAR1 + SPDK registered) |
| streams | [GpuStream; 2] | Alternating CUDA streams for D2D copies |
| slot_size | usize | Size of each staging slot in bytes |
| total_slots | usize | Number of slots (64) |

**Lifecycle**: Created once at initialization, destroyed at shutdown.

**Invariants**:
- All slots have valid GPU device pointers, BAR1 mappings, and SPDK DMA registrations
- Slots are never reallocated during component lifetime
- On drop: all GPU memory freed, BAR1 unmapped, SPDK unregistered, streams destroyed

### ThreadPartition

Per-thread view into the P2P ring for lock-free concurrent access.

| Field | Type | Description |
|-------|------|-------------|
| ring_offset | usize | Starting slot index for this thread's partition |
| effective_qd | usize | Number of slots available to this thread (max 16) |

**Invariants**:
- Partitions are non-overlapping
- `ring_offset + effective_qd <= total_slots`
- Partition assignment is stable for the duration of a lookup

### PipelineRing (DRAM Fallback)

Pre-allocated DRAM buffers for the standard SSD→DRAM→GPU path (reused from standard dispatcher).

| Field | Type | Description |
|-------|------|-------------|
| buffers | Vec\<Arc\<Mutex\<DmaBuffer\>\>\> | CUDA-pinned DRAM DMA buffers |
| streams | [GpuStream; 2] | Alternating CUDA streams for H2D copies |
| chunk_size | usize | Size of each buffer |

### LookupResult (from IDispatchMap)

Routing information returned by the dispatch map.

| Variant | Fields | Meaning |
|---------|--------|---------|
| NotExist | — | Key unknown |
| Staging | buffer: Arc\<DmaBuffer\> | Write in progress |
| BlockDevice | offset: u64 | Evicted to SSD at this LBA offset |
| MemoryTier | pointer: \*mut u8, size: u32 | Resident in DRAM |

### PathSelection

One-time decision stored for component lifetime.

| Variant | Meaning |
|---------|---------|
| P2p(P2pRing) | P2P ring available, cold reads use GPU staging |
| DramFallback(PipelineRing) | P2P unavailable, cold reads use DRAM bounce |

**Storage**: `OnceLock<PathSelection>` — set during initialization, immutable thereafter.

## State Transitions

### Entry Lifecycle (cold read)

```
BlockDevice (on SSD)
    │
    ├─ P2P path: NVMe read → P2pRing slot → D2D copy → client GPU
    │     │
    │     └─ promote → MemoryTier (back in DRAM)
    │
    └─ DRAM fallback: NVMe read → PipelineRing buffer → H2D copy → client GPU
          │
          └─ promote → MemoryTier (back in DRAM)
```

### Ring Slot Lifecycle (per cold read chunk)

```
Free → NVMe read submitted → NVMe complete → D2D copy issued → Stream synced → Free
```

## Relationships

- `P2pRing` owns `DmaBuffer` slots created via `gpu-services` `create_spdk_dma_buffer_from_cuda_malloc`
- `DispatcherP2pComponent` owns the `OnceLock<PathSelection>` deciding which ring is active
- `IDispatchMap::lookup()` returns `LookupResult::BlockDevice` for cold entries, triggering the cold path
- After successful cold read + copy, the entry is promoted back to `IMemoryTier` via `insert()`
