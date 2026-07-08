# Implementation Plan: Dispatcher-P2P (GPUDirect Cold-Path Dispatcher)

**Branch**: `001-dispatcher-p2p` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The `dispatcher-p2p` component is the GPU-direct peer-to-peer variant of the Certus cache dispatcher. It orchestrates data movement across a three-tier storage hierarchy (GPU VRAM, DRAM memory-tier, NVMe SSDs) with two distinct cold-read paths: a P2P path that bypasses host DRAM entirely (NVMe -> GPU BAR1 -> GPU VRAM) and a DRAM fallback path (NVMe -> DRAM -> GPU). The component manages background write-through, DRAM backfill after P2P reads, memory-tier eviction under pressure, SSD capacity eviction, and crash-consistent extent allocation across multi-drive configurations.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75

**Primary Dependencies**:
- `component-framework` / `component-core` / `component-macros` -- COM-inspired component model
- `interfaces` (spdk feature) -- Shared trait definitions (`IDispatcher`, `IBlockDevice`, `IExtentManager`, etc.)
- `gpu-services` (spdk, gpu, p2p features) -- CUDA FFI, GDRCopy BAR1 buffer creation, stream management
- `spdk-env` -- SPDK environment initialization wrapper
- `block-device-spdk-nvme` (optional, `spdk-backend` feature) -- NVMe block device actor driver
- `extent-manager` -- Fixed-size extent allocator with crash-consistent on-disk layout
- `disk-partition-manager` -- GPT partition table management per drive
- `memory-tier` -- DRAM slab allocator with LRU eviction support
- `crossbeam-channel` -- Lock-free MPSC channels for background worker communication
- `parking_lot` -- High-performance `RwLock` for shared state

**Performance Goals**:
- Hot path (DRAM -> GPU): Zero allocations on critical path; latency dominated by H2D DMA transfer time
- Cold path (P2P): Eliminate DRAM bounce; NVMe -> GPU BAR1 in single PCIe hop plus one intra-GPU copy
- Multi-drive: Linear throughput scaling for cold reads (one pipeline thread per drive)
- NVMe saturation: Maintain 16 in-flight commands per drive (full ring partition)
- Background write-through: Non-blocking populate path; persistence happens asynchronously

## Architecture

### Component Layer

```
                         +-----------------------+
                         |   GPU Inference       |
                         |   Client (CUDA)       |
                         +----------+------------+
                                    |
                         IPC Handle (GPU ptr + size)
                                    |
                         +----------v------------+
                         |  DispatcherP2pComponent|
                         |  (IDispatcher impl)   |
                         +--+-----+-----+-----+--+
                            |     |     |     |
              +-------------+  +--+--+  |  +--+------------+
              |                |     |  |  |               |
   +----------v---+   +-------v-+ +-v--v--v-+   +---------v--------+
   | IDispatchMap  |   |IMemory  | |IGpu     |   | ISPDKEnv         |
   | (dispatch-map)|   |Tier     | |Services |   | (device enum)    |
   +---------------+   +---------+ +---------+   +------------------+

   +---------------------------------------------------------------+
   |                    Data Drives (1..N)                          |
   |  +------------------+  +------------------+                   |
   |  | IBlockDevice     |  | IExtentManager   |                   |
   |  | (NVMe actor)     |  | (extent alloc)   |                   |
   |  +------------------+  +------------------+                   |
   +---------------------------------------------------------------+

   +---------------------------------------------------------------+
   |                  Background Subsystems                         |
   |  +--------------------+  +------------------+  +------------+ |
   |  | ParallelBackground |  | DramBackfill     |  | Background | |
   |  | Writer (per-drive) |  | Worker (per-drv) |  | Evictor    | |
   |  +--------------------+  +------------------+  +------------+ |
   +---------------------------------------------------------------+

   +---------------------------------------------------------------+
   |                  P2P Cold-Read Subsystem                       |
   |  +------------------+  +------------------+  +-------------+  |
   |  | P2pRing          |  | P2pColdReadPool  |  | PipelineRing|  |
   |  | (64 BAR1 slots)  |  | (worker threads) |  | (16 DMA buf)|  |
   |  +------------------+  +------------------+  +-------------+  |
   +---------------------------------------------------------------+
```

### Internal Module Structure

```
components/dispatcher-p2p/
  Cargo.toml
  CLAUDE.md
  .specify/specs/001-dispatcher-p2p/
    spec.md
    plan.md          <-- this file
    tasks.md
  src/
    lib.rs           -- Component definition, IDispatcher impl, initialization,
                        shutdown, lookup/populate/remove/batch logic, eviction,
                        write-through orchestration, PCI parsing, NUMA assignment
    background.rs    -- BackgroundWriter, ParallelBackgroundWriter (per-drive),
                        DramBackfillWorker, BackgroundEvictor, EvictorConfig
    cold_pool.rs     -- P2pColdReadPool: persistent worker threads with
                        pre-connected NVMe channels for cold-path dispatch
    io_segmenter.rs  -- MDTS-aware I/O segmentation (IoSegment, segment_io)
    p2p_ring.rs      -- P2pRing (64 GPU BAR1 staging buffers), ThreadPartition
    pipeline.rs      -- Pipelined transfer functions:
                          pipelined_ssd_to_gpu (ring-buffered)
                          pipelined_ssd_to_gpu_zero_copy (direct mem-tier)
                          pipelined_ssd_to_gpu_p2p (BAR1 ring, no DRAM bounce)
                          pipelined_multi_object_p2p (batched P2P)
                          pipelined_multi_object_zero_copy (batched DRAM)
                          pipelined_ssd_to_dram_only (promote without GPU)
                          pipelined_multi_ssd_to_dram_only (batched promote)
  benches/
    ssd_evictor_benchmark.rs
    pipeline_hw_benchmark.rs   (requires hardware-test feature)
    dispatcher_hw_benchmark.rs (requires hardware-test feature)
```

### Data Flow / Key Paths

**Hot Path (Memory-Tier Hit)**:
```
lookup(key, ipc) -> dispatch_map.lookup(key) -> MemoryTier{pointer, size}
  -> warm_stream (AtomicU64, lock-free load)
  -> gpu.memcpy_h2d_async(pointer, ipc.address, size, warm_stream)
  -> release_read(key), mt.touch(key)
  -> return stream for caller sync
```

**Cold Path - P2P (BAR1, no DRAM bounce)**:
```
lookup(key, ipc) -> dispatch_map.lookup(key) -> BlockDevice{offset}
  -> promote_and_serve(key, offset, ipc, ...)
  -> P2P ring available:
     -> pipelined_ssd_to_gpu_p2p(drive, ring, partition, channels, gpu_dst, lba, bytes)
        1. Prime: submit effective_qd NVMe ReadAsync into BAR1 ring slots
        2. On completion: cudaMemcpyAsync(gpu_dst+off, ring_slot, D2D, stream[i%N])
        3. Recycle: sync streams at interval, resubmit next read
        4. Final: sync all streams
     -> release_write(key)
     -> enqueue DramBackfillJob (async DRAM backfill)
```

**Cold Path - DRAM Fallback (no GDRCopy/BAR1)**:
```
lookup(key, ipc) -> BlockDevice{offset}
  -> promote_and_serve -> P2P ring NOT available:
     -> evict_for_space(dm, mt, size, key)
     -> mt.insert(key, size) -> mem_ptr
     -> pipelined_ssd_to_gpu_zero_copy(drive, gpu, streams, channels, mem_ptr, gpu_dst, ...)
        1. Wrap mem_tier chunks as DmaBuffer (noop_free)
        2. Prime: submit NVMe ReadAsync with tag = segment index
        3. On completion: dma_copy_to_device_async(chunk, gpu_dst+off, stream)
        4. Sync streams periodically, resubmit next read
     -> Register as MemoryTier in dispatch-map
```

**Populate Path (GPU -> DRAM -> SSD)**:
```
populate(key, ipc) ->
  Phase 1: reserve_memory(key, size)
    -> evict_for_space(dm, mt, size, key)
    -> mt.insert(key, size) -> mem_ptr
  Phase 2: copy_gpu_to_memory_async(key, ipc, warm_stream)
    -> gpu.dma_copy_to_host_async(ipc.address, mem_ptr_buf, size, stream)
  Phase 3: copy_gpu_to_memory_completed(key, size)
    -> dm.create_memory_tier_entry(key, mem_ptr, size)
    -> dm.downgrade_reference(key)  (write -> read for bg writer)
    -> bg_writer.enqueue(WriteJob{key, size, drive_index})
```

**Background Write-Through (per-drive worker)**:
```
process_write_job(dm, mt, drives, extent_mgrs, job):
  -> mt.peek(job.key) -> (mem_ptr, size)  [no LRU refresh]
  -> DmaBuffer::from_raw(mem_ptr, noop_free)
  -> extent_mgr.reserve_extent(key, aligned_size) -> WriteHandle
  -> write_buffer_to_ssd(drive, buf, start_lba, total_bytes, dma_capable)
     -> io_segmenter::segment_io(start_lba, bytes, max_transfer, sector_size)
     -> For each segment: send Command::WriteSync, recv Completion::WriteDone
  -> write_handle.publish()
  -> dm.convert_to_storage(key, block_offset)
  -> dm.release_read(key)
```

**Batch Lookup**:
```
batch_lookup(entries):
  1. Classify: dispatch_map.lookup each key
     - MemoryTier: serve immediately (memcpy_h2d_async on warm_stream)
     - BlockDevice: accumulate as ColdEntry
     - NotExist: mark KeyNotFound
  2. Group cold entries by drive (splitmix64 hash % num_drives)
  3. For each drive: submit to P2pColdReadPool (or inline fallback)
     -> pipelined_multi_object_p2p(drive, ring, partition, channels, jobs)
  4. Collect results, enqueue DramBackfillJobs for successful P2P reads
  5. Forward KeyNotFound entries to remote_lookup receptacle (if bound)
```

**Memory-Tier Eviction**:
```
evict_for_space(dm, mt, needed, target_key):
  while mt.used() + needed > mt.capacity():
    if attempts % 8 == 0:
      -> mt.oldest_keys(4) -> find key where dm.is_evictable(key)
      -> mt.remove(key), dm.convert_memory_tier_to_block(key)
    else:
      -> mt.evict_lru_for_key(target_key) -> evicted_key
      -> dm.convert_memory_tier_to_block(evicted_key)
    if attempts > 512: return AllocationFailed
```

**SSD Eviction (background)**:
```
evictor_loop:
  loop:
    sleep(interval)
    compute_utilization(extent_mgrs) -> (used, capacity)
    if used/capacity < threshold: continue
    dm.oldest_keys(batch_size) -> candidates
    for key in candidates:
      get_evictable_offset(dm, key) -> BlockDevice only
      mt.remove(key), dm.remove(key), extent_mgr.remove_extent(offset)
      if utilization < low_watermark: break
```

### Key Design Decisions

1. **P2P Ring Partitioning**: The 64-slot ring is statically partitioned across threads (ThreadPartition). With 4 drives x 1 queue thread each, every thread gets exactly 16 slots -- matching NVMe queue depth for PCIe bandwidth saturation. This eliminates runtime slot contention without locks.

2. **Three-Phase Shutdown**: Block device actors are torn down in order: (1) signal all actors to stop, (2) join all actor threads, (3) detach controllers. This prevents SPDK transport teardown from invalidating memory that actors are still polling.

3. **Lock-Free Warm Stream**: The CUDA stream for hot-path H2D copies is stored as `AtomicU64` (pointer cast). This avoids taking any lock on the hot path -- the only cost is an atomic load with Acquire ordering.

4. **noop_free DmaBuffer Wrappers**: Memory-tier pointers are wrapped in `DmaBuffer` for SPDK/CUDA APIs without transferring ownership. The `noop_free` function prevents double-free; wrappers are explicitly `std::mem::forget`'d after use.

5. **Hybrid Eviction Strategy**: Every 8th eviction attempt uses targeted lookup (`oldest_keys` + `is_evictable`) to find entries with completed SSD write-through. Other attempts use `evict_lru_for_key` which can evict any entry. This balances efficiency (targeted finds good candidates) with progress (LRU always frees something).

6. **Background Writer Per Drive**: One dedicated writer thread per NVMe drive enables concurrent write-through without lock contention on the I/O path. Jobs are routed by `drive_index` from the splitmix64 hash.

7. **Cold-Read Worker Pool**: `P2pColdReadPool` pre-connects NVMe `ClientChannels` at initialization. This eliminates per-batch connection overhead (~50us per connect) which would dominate for small batches.

8. **DRAM Backfill Delay**: After serving a P2P cold read, the backfill worker sleeps `backfill_delay_ms` before reading from SSD into DRAM. This avoids thrashing when the same key is re-requested before the backfill completes.

9. **splitmix64 Key Distribution**: Uses the splitmix64 finalizer (mix64 from SplitMix64 PRNG) on cache keys to distribute entries uniformly across drives, even for sequential key patterns.

10. **Factory-Based Component Creation**: Block device and extent manager instances are created via pluggable factories (`BlockDeviceFactory`, `ExtentManagerFactory`). This enables unit testing without SPDK hardware and supports alternate storage backends.

11. **NUMA-Aware CPU Pinning**: When `poller_base_cpu` is not explicitly set, the dispatcher queries SPDK device topology and assigns NVMe poller threads to CPUs on the same NUMA node as each drive, using round-robin allocation.

12. **Pipeline Telemetry**: The `pipeline-telemetry` feature flag enables per-segment timing breakdowns (submit, recv_wait, gpu_dma, sync, resub) for performance profiling without production overhead.

## Dependencies

| Dependency | Type | Integration |
|---|---|---|
| `IDispatchMap` | Receptacle | Tracks entry locations, reference counting, state transitions |
| `IMemoryTier` | Receptacle | DRAM slab allocator, LRU eviction, pool registration |
| `IGpuServices` | Receptacle | CUDA DMA, stream management, BAR1 buffer creation, host memory registration |
| `ISPDKEnv` | Receptacle | NVMe device enumeration, SPDK environment context |
| `ILogger` | Receptacle | Structured logging for all lifecycle events |
| `IRemoteLookup` | Receptacle (optional) | Distributed cache resolution for local misses |
| `IBlockDevice` | Created internally | Per-drive NVMe actor (via factory or SPDK backend) |
| `IExtentManager` | Created internally | Per-drive extent allocation with crash recovery |
| `DiskPartitionManager` | Created internally | GPT partition table per drive |

## Testing

### Unit Tests (in `src/lib.rs`)

- **Pre-initialization guards**: All IDispatcher methods return `NotInitialized` before `initialize()`
- **Lifecycle**: `populate` -> `check` -> `lookup` -> `remove` full cycle
- **Parameter validation**: Zero-size reject, duplicate key reject
- **Eviction**: Pool-full triggers eviction, no-op when space available, bounded scan (MAX_ATTEMPTS)
- **Concurrency**: Multi-threaded populate, concurrent checks, pre-init from multiple threads
- **Background evictor**: Evictable offset filtering, full eviction cycle, active reference protection, start/shutdown
- **Reinitialize after shutdown**: State reset verified

### Module-Level Tests

- `p2p_ring::tests`: Partition non-overlapping, single-thread max QD, 8-thread cap, bounds within ring
- `pipeline::tests`: Pipeline ring size sanity check
- `io_segmenter::tests`: Single segment, exact MDTS boundary, splits, uneven split, zero bytes, many segments, non-zero start LBA, panics on zero params
- `background::tests`: Start/shutdown, job processing, drain on shutdown, concurrent enqueue, slow processing, drop triggers shutdown

### Benchmarks

- `ssd_evictor_benchmark`: Criterion benchmark for SSD eviction hot loop
- `pipeline_hw_benchmark`: Hardware-gated benchmark for P2P pipeline throughput
- `dispatcher_hw_benchmark`: Hardware-gated end-to-end dispatcher performance

### Test Infrastructure

- `MockMemoryTier`: In-memory slab with configurable capacity, `fail_insert` mode
- `MockDispatchMap`: HashMap-based dispatch map with reference counting, mismatch injection
- `MockGpuServices`: CPU memcpy simulating CUDA DMA (enables testing without GPU)
- `MockLogger`: Silent logger for test noise reduction

## Future Considerations

- **Adaptive P2P/DRAM path selection**: Currently path is fixed at init; could dynamically switch based on observed latency or BAR1 contention.
- **Multi-GPU support**: Current ring is allocated on a single GPU; multi-GPU inference would need per-GPU rings and routing.
- **Speculative prefetch integration**: `promote_to_memory_tier` exists but is caller-driven; could add ML-based prefetch heuristics.
- **Compression**: SSD-stored extents could be compressed to increase effective capacity; decompress in P2P ring or DRAM path.
- **Tiered eviction policies**: Current LRU-based eviction could incorporate access frequency (LFU) or cost-aware policies.
- **Telemetry export**: `pipeline-telemetry` feature writes to stderr; could integrate with metrics/tracing framework.
- **Hot-standby**: Remote lookup currently delegates misses; could replicate hot entries to peer nodes proactively.
