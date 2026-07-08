# Implementation Plan: Dispatcher Component

**Branch**: `001-dispatcher` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The Dispatcher is the central data-plane orchestrator for Certus. It implements the `IDispatcher` interface to coordinate GPU-to-SSD cache operations through a DRAM memory-tier with LRU eviction and write-through persistence to NVMe SSDs. The component manages N data block devices with N extent managers for striped persistent storage, a persistent cold-path worker pool for SSD-to-GPU promotions, and a background evictor for SSD space reclamation.

## Technical Context

- **Language**: Rust stable, edition 2021, MSRV 1.75
- **Framework**: Component-framework (`define_component!` macro)
- **Crate name**: `dispatcher` (version 1.0.0)
- **Workspace membership**: Non-default member (requires SPDK or mock factories)
- **Feature gates**:
  - `spdk-backend` (default) -- enables real NVMe block device via `block-device-spdk-nvme`
  - `hardware-test` -- enables integration tests requiring real NVMe drives + GPUs
  - `pipeline-telemetry` -- enables stderr timing output for pipeline debugging
- **Key external crates**: `crossbeam-channel` (lock-free MPMC), `parking_lot` (efficient RwLock), `libc` (posix_memalign fallback)

## Architecture

### Component Layer

```
                    ┌─────────────────────────────────────────────────────┐
                    │              DispatcherComponent                     │
                    │  provides: IDispatcher                              │
                    │                                                     │
                    │  receptacles:                                       │
                    │    logger ──────────── ILogger                      │
                    │    dispatch_map ────── IDispatchMap                  │
                    │    gpu_services ────── IGpuServices                  │
                    │    spdk_env ────────── ISPDKEnv                      │
                    │    memory_tier ─────── IMemoryTier                   │
                    │    remote_lookup ───── IRemoteLookup (optional)      │
                    └──────────┬──────────────────────────────────────────┘
                               │
              ┌────────────────┼─────────────────────────────┐
              │                │                              │
     ┌────────▼────────┐  ┌───▼────────────┐  ┌─────────────▼───────────┐
     │  DataDrive[0..N] │  │ ParallelBg-    │  │  ColdReadPool           │
     │  (IBlockDevice + │  │ Writer (1/drv) │  │  (2 workers/drive)      │
     │   IExtentManager │  │ [WriteJob ch]  │  │  [pre-connected NVMe    │
     │   + channels)    │  └────────────────┘  │   + CUDA streams]       │
     └──────────────────┘                      └─────────────────────────┘
              │
     ┌────────▼────────┐
     │ BackgroundEvictor│
     │ (SSD util check) │
     └──────────────────┘
```

### Internal Module Structure

```
components/dispatcher/
├── Cargo.toml
├── CLAUDE.md
├── src/
│   ├── lib.rs              # DispatcherComponent, define_component!, IDispatcher impl
│   │                        # - DataDrive struct
│   │                        # - BlockDeviceFactory / ExtentManagerFactory types
│   │                        # - initialize / shutdown lifecycle
│   │                        # - populate / lookup / lookup_async / batch_lookup
│   │                        # - check / remove / touch
│   │                        # - reserve_memory / copy_gpu_to_memory_async / completed / release
│   │                        # - promote_to_memory_tier / clear_memory_tier / flush_to_ssd
│   │                        # - evict_for_space (LRU eviction strategy)
│   │                        # - write_buffer_to_ssd (MDTS-segmented writes)
│   │                        # - promote_and_serve (cold single-key promotion)
│   │                        # - drive_index (splitmix64 hash)
│   │                        # - create_data_drives / create_block_device
│   │                        # - compute_numa_cpu_assignments
│   │                        # - process_write_job
│   │                        # - Unit tests (mock infrastructure + 40+ test cases)
│   ├── background.rs       # BackgroundWriter, ParallelBackgroundWriter, BackgroundEvictor
│   ├── cold_pool.rs        # ColdReadPool (persistent worker pool)
│   ├── io_segmenter.rs     # MDTS-aware I/O segment splitting
│   ├── pipeline.rs         # PipelineRing, pipelined_ssd_to_gpu_zero_copy,
│   │                        # pipelined_multi_object_zero_copy,
│   │                        # pipelined_ssd_to_dram_only,
│   │                        # pipelined_multi_ssd_to_dram_only
│   └── metrics.rs          # PipelineMetrics trait
├── benches/
│   ├── dispatcher_benchmark.rs
│   ├── ssd_evictor_benchmark.rs
│   ├── pipeline_hw_benchmark.rs
│   └── dispatcher_hw_benchmark.rs
└── .specify/specs/001-dispatcher/
    ├── spec.md
    ├── plan.md             # (this file)
    └── tasks.md
```

### Data Flow / Key Paths

**Populate (GPU -> DRAM -> SSD)**:
1. `populate(key, ipc_handle)` validates params (non-zero size, non-duplicate key)
2. `reserve_memory(key, size)` evicts LRU entries if pool is full, then calls `mt.insert(key, size)`
3. `copy_gpu_to_memory_async(key, ipc_handle, stream)` wraps memory-tier slot as DmaBuffer, issues `dma_copy_to_host_async` from GPU source
4. `gpu.stream_synchronize(stream)` blocks until D2H DMA completes
5. `copy_gpu_to_memory_completed(key, size)` registers in dispatch-map, downgrades write ref to read ref, enqueues `WriteJob` to `ParallelBackgroundWriter`
6. Background writer: `process_write_job` peeks memory-tier pointer, reserves extent, writes MDTS-segmented I/O, publishes extent, calls `dm.convert_to_storage(key, offset)`

**Hot Lookup (DRAM -> GPU)**:
1. `batch_lookup` classifies each key via `dm.lookup(key)`
2. MemoryTier hits: `gpu.memcpy_h2d_async(pointer, gpu_dst, size, warm_stream)` round-robin
3. After all hot entries enqueued: single `gpu.stream_synchronize(warm_stream)` + `mt.batch_touch(keys)`

**Cold Lookup (SSD -> DRAM -> GPU)**:
1. BlockDevice hits collected during classification phase
2. Grouped by target drive (via `drive_index` hash)
3. Per-drive chunks submitted to `ColdReadPool` workers (or inline fallback)
4. Each worker: `evict_for_space` + `mt.insert` + `pipelined_multi_object_zero_copy`
   - NVMe reads directly into CUDA-pinned memory-tier slot (zero-copy)
   - Each NVMe completion triggers `gpu.dma_copy_to_device_async` on dual alternating streams
   - Periodic stream sync every 8 completions bounds GPU queue depth
5. Results collected; dispatch-map updated: `dm.remove(key)` + `dm.create_memory_tier_entry` + `dm.convert_to_storage`

**Remote Lookup Fallback**:
1. After hot + cold phases, any remaining `KeyNotFound` entries forwarded to `IRemoteLookup.batch_lookup`
2. Remote results merged into final result vector

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| Zero-copy cold path via SPDK+CUDA co-registration | Eliminates CPU memcpy: NVMe DMA directly into CUDA-pinned memory-tier slot, then async H2D from same memory |
| Persistent ColdReadPool (pre-connected NVMe channels + CUDA streams) | Eliminates per-batch connection setup overhead (~100us savings per cold batch) |
| Splitmix64 hash for drive selection | Uniform distribution of sequential keys across N drives; deterministic for reproducibility |
| Per-drive ParallelBackgroundWriter | Concurrent write-through across N drives without head-of-line blocking |
| Dual CUDA streams with periodic sync (every 8 completions) | Overlaps GPU DMA transfers while bounding queue depth; avoids unbounded memory pressure |
| Tag-based NVMe completion routing | Enables out-of-order completion handling in multi-object pipeline; tag encodes `(obj_idx, seg_idx)` |
| Two-phase block device shutdown (signal-all, join-all, detach-all) | Prevents use-after-free when SPDK transport teardown invalidates memory |
| Alternating eviction strategy (targeted LRU + batch scanning every 8th attempt) | Balances O(1) fast-path eviction with finding cleanly-evictable entries (write-through complete) |
| `noop_free` DmaBuffer wrappers | Memory-tier owns allocations; DmaBuffer wrappers must not free them |
| Configurable factories (BlockDeviceFactory, ExtentManagerFactory) | Enables mock-based unit testing without SPDK hardware |
| Feature-gated SPDK backend | Crate builds without SPDK for CI; hardware tests opt-in via `hardware-test` feature |

## Dependencies

### Build Dependencies

| Crate | Purpose |
|-------|---------|
| `component-framework` | Component model facade (re-exports core + macros) |
| `component-core` | IUnknown, binding, receptacles, NUMA topology |
| `component-macros` | `define_component!` proc macro |
| `interfaces` (features: spdk) | IDispatcher, IBlockDevice, IExtentManager, IDispatchMap, IGpuServices, IMemoryTier, IRemoteLookup, ILogger |
| `gpu-services` (features: spdk, gpu) | GPU services interface definitions |
| `spdk-env` | ISPDKEnv trait and SPDKEnvComponent stub |
| `block-device-spdk-nvme` (optional) | Real NVMe block device driver via SPDK |
| `disk-partition-manager` | GPT partition table management per drive |
| `extent-manager` | Fixed-size extent allocator with crash-consistent metadata |
| `memory-tier` | DRAM pool management with LRU eviction |
| `crossbeam-channel` | Lock-free MPMC channels for background writer and cold pool |
| `parking_lot` | Efficient RwLock for data_drives and pipeline_ring |
| `libc` | `posix_memalign` fallback for non-SPDK path |

### Dev Dependencies

| Crate | Purpose |
|-------|---------|
| `criterion` | Benchmark harness |
| `eviction-policy-lru` | LRU policy for integration tests |
| `logger` | Console logger for integration tests |

### Runtime Component Dependencies (receptacles)

| Receptacle | Interface | Required | Notes |
|------------|-----------|----------|-------|
| `logger` | ILogger | Yes | Structured logging |
| `dispatch_map` | IDispatchMap | Yes | Key-to-location mapping |
| `gpu_services` | IGpuServices | Yes | GPU DMA, CUDA streams |
| `spdk_env` | ISPDKEnv | No* | NVMe device discovery (*required for real hardware) |
| `memory_tier` | IMemoryTier | Yes | DRAM pool allocation + LRU |
| `remote_lookup` | IRemoteLookup | No | Optional multi-node forwarding |

## Testing

### Unit Tests (in lib.rs)

| Category | Count | Description |
|----------|-------|-------------|
| Pre-initialization | 9 | All operations return NotInitialized before init |
| Initialization | 4 | Config validation, receptacle wiring, multiple PCI addrs |
| Populate | 6 | Success, zero-size, duplicate key, allocation failure, non-aligned, batch |
| Lookup | 4 | Memory-tier hit, cold promote, not found, size mismatch |
| Check/Remove | 4 | Existing/nonexistent keys |
| Lifecycle | 3 | Full cycle, post-shutdown, re-initialization |
| Concurrency | 3 | Concurrent pre-init, checks, populates |
| Eviction | 3 | Pool full, noop, populate-triggered |
| Background Evictor | 5 | Offset resolution, eviction cycle, active refs, start/stop |
| Promote | 4 | Block->memory, already-hot noop, nonexistent, mixed batch |

### Module-Level Tests

| Module | Tests | Description |
|--------|-------|-------------|
| `background.rs` | 5 | Writer start/stop, job processing, concurrent enqueue, drain, drop |
| `io_segmenter.rs` | 8 | Segment splitting, boundaries, zero-byte, panics |
| `pipeline.rs` | 1 | Ring size sanity check |

### Benchmark Suites

| Benchmark | Feature Gate | Description |
|-----------|-------------|-------------|
| `dispatcher_benchmark` | none | Mock-based populate/lookup/check/remove throughput |
| `ssd_evictor_benchmark` | none | Evictor decision throughput |
| `pipeline_hw_benchmark` | hardware-test | Real NVMe+GPU pipeline bandwidth |
| `dispatcher_hw_benchmark` | hardware-test | End-to-end hardware throughput |

### Test Strategy

- **Mock-based**: All unit tests use mock implementations (MockMemoryTier, MockDispatchMap, MockGpuServices) enabling testing without hardware
- **Staging-only mode**: When ISPDKEnv is not connected and no factory is set, the dispatcher operates in a degraded mode suitable for testing core logic
- **Hardware integration**: `--features hardware-test` activates tests that use real NVMe devices and GPUs

## Future Considerations

1. **Formal verification**: The spec references properties P1-P10 in `components/dispatcher/verif/` but that directory does not yet exist. Formal models (e.g., Spin/Promela) should verify invariants like "a key never exists in both MemoryTier and BlockDevice states simultaneously."

2. **Per-shard eviction**: The current eviction strategy handles global capacity pressure but may struggle when keys are heavily skewed to one shard. A per-shard eviction path could improve behavior under skewed workloads.

3. **Metrics instrumentation**: The `PipelineMetrics` trait is defined but integration with an observability backend (OpenTelemetry) is external to this component. The certus-server layer implements this.

4. **Crash recovery completeness**: The dispatch-map rebuild from on-disk extents recovers keys in BlockDevice state. Entries that were in MemoryTier at crash time are lost (acceptable since DRAM is volatile), but the code path should be verified under power-loss scenarios.

5. **Multi-GPU support**: The current design uses a single `warm_stream` and single GPU context. Multi-GPU topologies would require per-GPU stream allocation and GPU-aware key routing.

6. **Tiered eviction priority**: The SSD evictor currently removes oldest BlockDevice entries uniformly. A frequency-aware policy (LFU hybrid) could improve hit rates for access patterns with temporal locality.

7. **Connection pooling for remote lookup**: The `IRemoteLookup` receptacle is single-instance; high-throughput deployments may need connection pooling or sharded forwarding.
