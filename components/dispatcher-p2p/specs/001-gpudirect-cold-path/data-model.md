# Data Model: GPUDirect Storage Cold Path

## Entities

### P2pRing

Pre-allocated ring of GPU-resident staging buffers for NVMe-to-GPU P2P transfers.

| Field | Type | Description |
|-------|------|-------------|
| slots | Vec\<DmaBuffer\> | 64 GPU staging buffers (cudaMalloc + BAR1 + SPDK registered) |
| streams | [GpuStream; 4] | Round-robin CUDA streams for D2D copies (`NUM_STREAMS = 4`; minimum 2 on constrained hardware) |
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

### PathSelection (conceptual — see Corrected Storage below)

Which cold-read path is active for the component's lifetime.

| Case | Meaning |
|------|---------|
| P2P ring present | P2P ring available, cold reads use GPU staging |
| P2P ring absent, pipeline ring present | P2P unavailable, cold reads use DRAM bounce |

**Corrected Storage** (2026-07-22 — supersedes the single-enum description above): the component does **not** store a single `OnceLock<PathSelection>` enum. It holds two independent fields, each set at most once during initialization and never mutated afterward:

| Field | Type | Description |
|-------|------|-------------|
| `p2p_ring` | `RwLock<Option<P2pRing>>` | Populated at init if P2P ring allocation succeeds |
| `pipeline_ring` | `RwLock<Option<PipelineRing>>` | Populated at init as the DRAM-bounce fallback ring |

Both fields *can* be populated simultaneously (e.g., a P2P-capable deployment still keeps a DRAM fallback ring for the single-key `lookup()` path per FR-006/FR-007). The active path per call is chosen by reading `p2p_ring`: `batch_lookup` requires `Some` and panics otherwise (FR-006); single-key `lookup()` falls back to `pipeline_ring`/DRAM if `p2p_ring` is `None`. There is no single "one-time decision" value — the choice is re-evaluated (cheaply, via `RwLock::read()`) on every call.

### P2pColdReadPool

Persistent pool of long-lived per-(drive, queue-slot) worker threads that execute cold-path P2P pipeline jobs, avoiding per-batch connection setup.

| Field | Type | Description |
|-------|------|-------------|
| workers | Vec\<Vec\<WorkerHandle\>\> | Outer index = drive, inner index = queue-slot (`MAX_QUEUES_PER_DRIVE` per drive) |
| shutdown | Arc\<AtomicBool\> | Shared shutdown flag observed by all worker loops |
| num_drives | usize | Number of drives the pool was built for |

**WorkerHandle**: `{ sender: Sender<P2pColdReadRequest>, _handle: JoinHandle<()> }` — one OS thread per (drive, queue-slot), each owning a pre-connected `ClientChannels` for its drive, created once in `P2pColdReadPool::new`.

**P2pColdReadRequest**: `{ jobs: Vec<P2pColdJob>, partition: ThreadPartition, ring_ptr: *const P2pRing, result_tx: Sender<Vec<Result<(), DispatcherError>>> }` — submitted over a bounded (depth-1) channel; the worker runs `pipelined_multi_object_p2p` and returns per-job results via `result_tx`.

**Lifecycle**: Created at initialization once the P2P ring is available and at least one drive is registered (`src/lib.rs:1239-1260`); on creation failure, the component logs a non-fatal warning and every cold `batch_lookup` falls back to the inline per-batch path (connect + run on the calling thread) for the component's remaining lifetime. Signaled to stop via `shutdown()` (sets the `AtomicBool`; each worker checks it before executing the next request and drains its channel with an error result) as part of component `shutdown()`, before the P2P ring is destroyed. Also stopped via `Drop`.

**Invariants**:
- At most one pool instance per component; guarded by `Mutex<Option<P2pColdReadPool>>`
- Worker count per drive is fixed at pool creation (`MAX_QUEUES_PER_DRIVE`, currently 1)
- `ring_ptr` in an in-flight request is valid for the request's duration because the caller holds a `p2p_ring` `RwLock` read guard until `result_tx` is received

### EvictionEvent / EvictionReason

Notification of a memory-tier eviction, published best-effort to an optional external subscriber.

| Field / Variant | Type | Description |
|------|------|-------------|
| `EvictionEvent.key` | CacheKey | Key that was evicted |
| `EvictionEvent.reason` | EvictionReason | `Demoted` (moved to block device) or `Removed` (dropped entirely) |

**Channel**: `create_eviction_channel(capacity) -> Receiver<EvictionEvent>` creates a bounded `crossbeam_channel` and stores the sender half in `eviction_tx: Arc<Mutex<Option<Sender<EvictionEvent>>>>` (single active subscriber; a later call replaces the previous sender). Every eviction performed via `evict_for_space_emit` (the DRAM-space-reclaim path used by lookups/writes) calls `try_send` on the registered sender, if any.

**Backpressure**: `try_send` failure (channel full) or no registered subscriber causes the event to be silently dropped; the drop count is accumulated in `eviction_dropped: AtomicU64` and exposed/reset via `eviction_dropped_count()`. Eviction event publication never blocks or fails the eviction itself.

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
- `DispatcherP2pComponent` owns the `p2p_ring: RwLock<Option<P2pRing>>` and `pipeline_ring: RwLock<Option<PipelineRing>>` fields whose presence/absence decides which cold path is active per call (see corrected `PathSelection` storage above)
- `IDispatchMap::lookup()` returns `LookupResult::BlockDevice` for cold entries, triggering the cold path
- After successful cold read + copy, the entry is promoted back to `IMemoryTier` via `insert()`
- `P2pColdReadPool` (when present) is the primary executor of the P2P path: `batch_lookup` submits `P2pColdReadRequest`s referencing the active `P2pRing`; on pool-creation failure it is `None` and the inline per-batch fallback is used instead
- `DispatcherP2pComponent` owns `eviction_tx: Arc<Mutex<Option<Sender<EvictionEvent>>>>` and `eviction_dropped: AtomicU64`; `evict_for_space_emit` publishes `EvictionEvent`s to `eviction_tx` as a side effect of DRAM-space reclamation, independent of which cold path serves the triggering lookup
