# Research: GPUDirect Storage Cold Path

## R1: P2P Staging Ring Allocation Strategy

**Decision**: Pre-allocate 64 GPU-resident staging buffers using `cudaMalloc` + GDRCopy `gdr_pin_buffer`/`gdr_map` + `spdk_mem_register`. Use `gpu-services` crate's `create_spdk_dma_buffer_from_cuda_malloc()` function which wraps this sequence.

**Rationale**: Client GPU memory arrives via `cudaIpcOpenMemHandle` (IPC). GDRCopy's `gdr_pin_buffer` returns EINVAL on IPC-opened memory — only locally `cudaMalloc`'d pointers work. Therefore we must stage through pre-allocated local buffers and D2D copy to the IPC destination. The D2D copy runs at GPU internal bandwidth (~600 GB/s), making the overhead negligible.

**Alternatives considered**:
- Per-call BAR1 mapping from client pointer: Rejected — GDRCopy EINVAL on IPC memory.
- Dynamically sized ring: Rejected — allocation jitter at runtime; fixed ring is predictable and avoids fragmentation.

## R2: Ring Partitioning for Concurrent Access

**Decision**: Split 64 slots into non-overlapping halves (or quarters under higher concurrency). Each thread gets a `ring_offset` into its partition. Effective queue depth per thread: 16 (64 / max_threads capped to prevent NVMe qpair saturation).

**Rationale**: Lock-free partitioning avoids contention on the hot path. With 4+ concurrent clients each submitting async NVMe reads, limiting per-thread queue depth to 16 prevents overwhelming NVMe qpair resources while maintaining full pipeline utilization.

**Alternatives considered**:
- Mutex-guarded shared ring: Rejected — contention under concurrent load defeats the purpose of pipelining.
- Per-thread dedicated rings: Rejected — wastes GPU memory proportional to thread count.

## R3: Pipeline Algorithm Design

**Decision**: Use a prime-and-replenish pipeline with alternating CUDA streams:
1. Prime: submit up to `effective_qd` async NVMe reads into ring slots
2. On each NVMe completion: issue D2D copy on alternating stream, submit next read into recycled slot
3. Sync both streams every `ring_size/2` completions before recycling
4. Finalize: sync both streams after all chunks complete

**Rationale**: Overlapping NVMe reads with D2D copies maximizes throughput. Alternating streams allows one copy to proceed while the next read lands. Periodic sync prevents overrunning slots that haven't finished copying.

**Alternatives considered**:
- Single stream with full sync after each read: Rejected — serializes read and copy, halving throughput.
- Three or more streams: Rejected — diminishing returns vs complexity; two streams already hide the D2D latency.

## R4: Path Selection Mechanism

**Decision**: Attempt `P2pRing::new()` at initialization. Store result in `OnceLock<Option<P2pRing>>`. If `Some`, cold lookups use P2P path. If `None`, use DRAM fallback. Immutable for component lifetime.

**Rationale**: One-time decision avoids per-lookup branching overhead. `OnceLock` is safe for concurrent reads after initialization. Failure reasons (GDRCopy unavailable, insufficient GPU memory) are logged at startup.

**Alternatives considered**:
- Per-lookup fallback with retry: Rejected — adds latency checking P2P availability on every cold read; hardware state won't change at runtime.
- Feature flag at compile time: Rejected — requires separate binaries; runtime detection is more flexible for deployment.

## R5: Graceful Fallback Implementation

**Decision**: On `P2pRing::new()` failure, the component operates identically to the standard dispatcher's cold path: NVMe read into memory-tier DRAM slot, then `cudaMemcpyAsync` H2D to client GPU. The existing `PipelineRing` (DRAM-based) from the standard dispatcher handles this path.

**Rationale**: No new code needed for the fallback — reuse the standard dispatcher's pipeline. Any partial P2P ring allocation is cleaned up in `P2pRing::new()`'s error path before returning `None`.

**Alternatives considered**:
- Separate fallback component: Rejected — unnecessary; same binary handles both paths.

## R6: Performance Measurement Approach

**Decision**: Two-tier measurement:
1. Criterion benchmarks in `benches/cold_path_benchmark.rs` for micro-level pipeline comparison (requires hardware)
2. `certus-api-bench_v2.py` for end-to-end system throughput under realistic hot/cold workloads

**Rationale**: Criterion provides repeatable, per-commit regression detection. The v2 benchmark provides system-level validation with multi-client concurrency and hot/cold mixes that exercise the full dispatcher path.

**Alternatives considered**:
- Only Criterion: Rejected — doesn't capture system-level behavior (IPC, server startup, client concurrency).
- Only certus-api-bench: Rejected — too coarse for detecting pipeline-level regressions.

## R7: Interface Contract with gpu-services

**Decision**: Use the following gpu-services APIs (all under the `p2p` feature):
- `create_spdk_dma_buffer_from_cuda_malloc(ptr, size)` — For staging ring slot creation
- `memcpy_h2d_async(src, dst, size, stream)` — For D2D copies (staging → client GPU)
- `create_stream()` / `stream_synchronize(stream)` / `destroy_stream(stream)` — CUDA stream management
- Standard `IGpuServices` methods for IPC handle deserialization and DMA buffer creation (hot path)

**Rationale**: The `gpu-services` crate already encapsulates all CUDA/GDRCopy/SPDK memory registration. The dispatcher-p2p component should not duplicate this logic (per constitution Principle VII).

**Alternatives considered**:
- Direct FFI to CUDA/GDRCopy: Rejected — duplicates gpu-services, violates maintainability principle.
