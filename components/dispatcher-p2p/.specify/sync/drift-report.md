# Spec Drift Report
Generated: 2026-06-18
Project: dispatcher-p2p

## Summary
| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 13 |
| Aligned | 12 (92%) |
| Drifted | 1 (8%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 5 |

## Detailed Findings
### Spec: 001-gpudirect-cold-path - GPUDirect Storage Cold Path

#### Aligned
- FR-001: System MUST read evicted data from SSD directly into GPU staging buffers, bypassing host DRAM. -> `src/pipeline.rs:703-861` (`pipelined_ssd_to_gpu_p2p` reads NVMe directly into P2P ring GPU BAR1 slots)
- FR-002: System MUST copy data from staging buffers to the client's GPU destination. -> `src/pipeline.rs:795-807` (D2D `cudaMemcpyAsync` from ring slot dev_ptr to `gpu_dst`)
- FR-003: System MUST pre-allocate a fixed ring of 64 GPU staging buffers at initialization via cudaMalloc + GDRCopy BAR1 mapping + spdk_mem_register. Each slot is 128 KiB (MDTS). -> `src/p2p_ring.rs:19,52-126` (P2P_RING_SLOTS=64, cudaMalloc + `create_spdk_dma_buffer_from_gpu_bar` per slot, 4 CUDA streams allocated)
- FR-004: System MUST partition the staging ring for concurrent thread access using ThreadPartition (non-overlapping slot ranges, effective QD capped at 16 per thread). -> `src/p2p_ring.rs:160-181` (ThreadPartition with MAX_QD_PER_THREAD=16, non-overlapping partitions)
- FR-005: System MUST pipeline SSD reads with D2D GPU copies using FIFO completion ordering. D2D copies distributed round-robin across 4 CUDA streams. Stream sync at ring partition wrap. Final sync after all chunks. -> `src/pipeline.rs:703-861` (FIFO ordering, round-robin stream assignment `completed % num_streams`, sync at `sync_interval`, final sync loop)
- FR-006: System MUST panic on first cold lookup if the P2P ring was not initialized. Initialization logs diagnostic but does not fail. No DRAM fallback. -> `src/lib.rs:974-989` (init logs warning on None), `src/lib.rs:1438-1439` (`p2p_ref.expect(...)` panics on first cold batch_lookup if ring is None)
- FR-007: The P2P ring is allocated once at initialization and is immutable for the component's lifetime. No runtime path selection. -> `src/lib.rs:974,149` (allocated once in `initialize()`, stored in `RwLock<Option<...>>`, never replaced)
- FR-008: System MUST implement the same interface as the standard dispatcher, serving as a drop-in replacement. -> `src/lib.rs:132-155` (`define_component!` provides `[IDispatcher]`; all `IDispatcher` trait methods implemented)
- FR-009: System MUST promote successfully read cold entries back to DRAM after completing the read. -> `src/lib.rs:1452-1478` (batch_lookup cold path inserts into memory-tier after P2P read, re-registers in dispatch-map)
- FR-010: System MUST release all staging resources on shutdown with no leaks. -> `src/lib.rs:1089-1170` (shutdown destroys warm stream, P2P ring `ring.destroy(&*gpu)`, pipeline ring, stops actor threads)
- FR-011: System MUST handle read failures gracefully without corrupting ring state or affecting other in-flight operations. -> `src/pipeline.rs:766-786` (P2P pipeline returns Err on read failure; ring slot is not recycled improperly; other chunks not affected)
- FR-013: System MUST implement promote_to_memory_tier(keys) using pipelined_ssd_to_dram_only. -> `src/lib.rs:1956-2099` (promote_to_memory_tier implementation uses `pipelined_ssd_to_dram_only`, one thread per drive, no P2P ring)

#### Drifted
- FR-012: Spec says "System MUST support end-to-end performance measurement using the existing pipelined benchmark tool" but code has no explicit benchmark integration or measurement hooks within this component.
  - Location: N/A (no bench code in src/)
  - Severity: minor
  - Notes: The component itself implements `IDispatcher` which the external benchmark tool (`certus-api-bench_v2.py`) exercises. The benchmark infrastructure exists externally (in `apps/` and `benches/`), and there is a `benches/` directory in the component. The spec requirement is satisfied at the system level rather than in-component, making this a minor documentation-level drift rather than a functional gap.

#### Not Implemented
(none)

## Unspecced Code
- **Background write-through to SSD**: `src/background.rs` - ParallelBackgroundWriter persists memory-tier entries to SSD asynchronously after populate(). Not specified in any FR-* requirement.
- **Background SSD evictor**: `src/background.rs:220-393` - BackgroundEvictor monitors SSD utilization and evicts cold extents when threshold exceeded. Not covered by any FR-*.
- **prepare_store / commit_store / cancel_store**: `src/lib.rs:1784-1943` - Direct-write path allowing callers to get a DMA buffer, write into it, then commit/cancel. Not mentioned in spec.
- **IO segmentation (MDTS-aware splitting)**: `src/io_segmenter.rs` - Splits large transfers into MDTS-sized segments. An implementation detail not specified as a requirement.
- **Pipeline zero-copy and multi-object pipelines**: `src/pipeline.rs:244-675` - `pipelined_ssd_to_gpu_zero_copy`, `pipelined_multi_object_zero_copy`, `pipelined_multi_ssd_to_dram_only` are additional pipeline variants beyond the P2P path. These serve the DRAM fallback path and batch operations.

---

## Previous History

### Resolved 2026-06-16

- DRIFT-A: P2P ring failure behavior -- spec and code both specify panic on first cold lookup, not at startup.
- DRIFT-B: `promote_to_memory_tier` unspecced -- Added FR-013 to spec.
- DRIFT-C: Thread topology and CUDA streams -- Updated FR-004 and FR-005.

### Resolved 2026-06-12

- DRAM fallback removed: fail-fast at startup (US2, FR-006, FR-007, SC-006)
- P2P ring uses real BAR1 (FR-003)
- Pipeline sync strategy aligned (FR-005)
- Performance measurement references standard dispatcher (US4, SC-005)
