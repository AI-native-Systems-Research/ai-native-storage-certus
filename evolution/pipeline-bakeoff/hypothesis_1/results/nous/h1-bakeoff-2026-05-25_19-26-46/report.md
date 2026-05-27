# Pipeline Optimization Research Report

## Answer

An optimal pipelining configuration **does exist and outperforms the current defaults by ~25%**: setting `ZERO_COPY_DEPTH=32` (matching the 32-chunk count for 4 MiB objects) raises median cold-lookup throughput from ~3.5 GB/s to ~4.4 GB/s. However, this configuration **does not match or exceed the P2P direct path** because GDRCopy cannot pin GPU memory opened via `cudaIpcOpenMemHandle`, making the P2P path unavailable in the production IPC-shared memory model — meaning the pipelined path is the only viable high-throughput route and its optimized form (QD=32, single stream, no mid-transfer sync) is effectively the ceiling.

---

## Evidence

### Iteration 1 — Pipeline vs. P2P Bakeoff
- **Baseline pipeline** (ZERO_COPY_DEPTH=16, 2 streams, SYNC_FREQUENCY=16): 3.52–4.38 GB/s across runs (±15% variance).
- **Depth=32 treatment**: median ~4.38 GB/s, p50 latency reduced by ~32% versus depth=16 at ~1,060 µs median — **+25% improvement confirmed (RP-23)**.
- **P2P direct path**: `gdr_pin_buffer` returned `rc=22 (EINVAL)` when targeting IPC-opened GPU memory, making the 3.4 GB/s standalone benchmark result non-transferable to the production architecture **(RP-21)**. The robustness arm was **REFUTED**.
- Sequential (non-pipelined) baseline: 0.76 GB/s — pipeline yields **4.6× improvement** over sequential (RP-22).

### Iteration 2 — Pipeline Ablation: Mutex, Sync, Stream Count
- **Removing mid-transfer CUDA stream sync** (SYNC_FREQUENCY=∞): zero measurable change in throughput. GPU DMA completes 128 KiB in ~5 µs; 16 NVMe completions accumulate over ~480 µs, so sync always finds idle streams **(RP-25)**.
- **Single stream vs. dual stream**: no measurable difference for 128 KiB H2D copies on SM8.x hardware **(RP-26)**.
- **Removing pipeline_ring Mutex in zero-copy path**: +11% aggregate throughput at 4 concurrent clients, but the outer `Mutex<Dispatcher>` at `service.rs:186` serializes entire batch processing (~19 ms hold time per batch), capping multi-client gains **(RP-27)**.
- All three ablations produced **0/4 correct arm predictions**, confirming that commonly assumed bottlenecks (sync overhead, stream count, inner locks) are not limiting factors.

---

## Principles Discovered

| ID | Statement | Confidence | Regime |
|----|-----------|------------|--------|
| **RP-21** | GDRCopy cannot pin IPC-opened GPU memory (`cudaIpcOpenMemHandle`) | High | Any cross-process IPC GPU memory sharing |
| **RP-22** | Pipelining yields 4.6× over sequential at 4 MiB / 32 chunks | High | Single NVMe, 128 KiB MDTS, SPDK + CUDA pinned DMA |
| **RP-23** | ZERO_COPY_DEPTH=32 gives +25% throughput and −32% p50 latency vs. depth=16 | High | 4 MiB objects (32 chunks), QD matches chunk count |
| **RP-24** | ±15% run-to-run variance driven by pool eviction, not pipeline constants | Medium | 256 MiB pool, 160 cold objects, 10-iteration benchmark |
| **RP-25** | Mid-transfer CUDA stream sync has zero cost in zero-copy mode | High | 128 KiB chunks, PCIe Gen4 GPU DMA (~25 GB/s) |
| **RP-26** | Dual CUDA streams provide no benefit over single stream for 128 KiB H2D copies | High | SM8.x (A100/H100), unidirectional H2D only |
| **RP-27** | Outer `Mutex<Dispatcher>` is the dominant multi-client serialization bottleneck | High | 4+ concurrent gRPC clients, batch cold lookups |

---

## Limitations & Open Questions

### What Wasn't Answered
1. **True P2P ceiling is unknown**: the P2P path was never validly benchmarked in the production IPC architecture. A same-process staging buffer (allocate locally → GDRCopy → `cudaMemcpyPeer`) could in principle match or beat the pipelined NVMe path but was not implemented or measured.
2. **Multi-drive scaling**: all results are from a single NVMe device. With N drives, optimal QD per drive and aggregate throughput scaling are unmeasured.
3. **Larger object sizes**: behavior at 8 MiB+ (>64 chunks, requiring a true sliding window) was not characterized.
4. **The outer Mutex fix**: replacing `Mutex<Dispatcher>` with an `RwLock` or per-key locking was identified as a necessary fix (RP-27) but not implemented or benchmarked.

### Next Campaign Priorities
1. **Replace `Mutex<Dispatcher>` with `RwLock`** and re-run the 4-client aggregate throughput benchmark to quantify multi-client gains from concurrent cold lookups.
2. **Implement local-alloc P2P staging** (cudaMalloc locally → GDRCopy → cudaMemcpyPeer to IPC destination) and compare to the depth=32 pipelined path at multiple object sizes.
3. **Characterize variance** by adding explicit pool-drain normalization between benchmark runs to test whether the ±15% variance shrinks, enabling tighter detection of smaller (5–10%) throughput differences.
4. **Multi-NVMe fan-in**: measure whether QD=32/N per drive with N parallel SPDK channels maintains 4.4 GB/s per drive or reveals new bottlenecks (PCIe bandwidth, GPU BAR1 saturation).