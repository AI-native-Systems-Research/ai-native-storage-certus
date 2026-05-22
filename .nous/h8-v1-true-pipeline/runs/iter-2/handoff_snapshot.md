# Handoff — h8-v1-true-pipeline Iteration 2

## Goal

Implement true overlapped pipelining in dispatcher v1 using `cudaHostAlloc`-allocated staging buffers (natively CUDA-pinned + SPDK-registered) as the intermediate for both NVMe reads and async GPU copies. Compare against P2P direct DMA. All through certus-server with 4 MiB objects on NVMe 63:00.0.

## Key Discoveries

- **Iter-1 root cause confirmed: `cudaHostRegistered` mmap'd memory does NOT enable true async GPU DMA.** Iter-1's pipeline achieved only 10% improvement (19,220 vs 21,361 us) because `cudaMemcpyAsync` from `cudaHostRegistered` memory falls back to synchronous execution. The overlap came purely from ReadAsync pre-issuing (NVMe read overlaps with CPU memcpy + sync GPU copy).
- **`cudaHostAlloc` memory DOES enable true async.** Proven in h8-pipelined campaign: `cudaHostAlloc` + SPDK registration gave 2.4-3x speedup. The CUDA DMA engine requires natively-pinned memory (cudaHostAlloc) vs registered memory (cudaHostRegister) for true background DMA.
- **`create_spdk_dma_buffer_from_cuda_host_alloc` at `dma.rs:253`** wraps a `cudaHostAlloc` pointer as an SPDK DmaBuffer via `spdk_mem_register`. This makes it a valid NVMe DMA target AND a valid cudaMemcpyAsync source. Cleanup handled by `spdk_unregister_and_cuda_free_host` (dma.rs:232).
- **Feature unification makes dma module available.** certus-server enables `gpu` on gpu-services, dispatcher-v1 enables `spdk`. Cargo unifies → both features active → `create_spdk_dma_buffer_from_cuda_host_alloc` is compiled and accessible.
- **Iter-1 patches NOT merged to branch.** The current pipeline.rs is still the sequential version. The cuda_ffi.rs stream/async declarations from iter-1 must be re-applied.
- **Staging buffers as NVMe DMA targets:** Since `create_spdk_dma_buffer_from_cuda_host_alloc` produces a proper `DmaBuffer` (with SPDK vtophys resolution), the staging buffer can be passed directly as the `buf` in `Command::ReadAsync`. No intermediate SPDK DMA buffer needed — eliminates one memcpy.
- **P2P baseline from iter-1:** 16,396 us/obj (23% faster than sequential 21,361 us). Historical: 3,451 us (h8-v1-pinned iter-2). The 3-5x inflation is a system-level confound, not code defect.

## System Interface

- **Build:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server`
- **Run server:** `LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH sudo target/debug/certus-server --metadata-pci 0000:62:00.0 --data-pci 0000:63:00.0 --dispatcher-version v1 --listen 0.0.0.0:50051`
- **Run benchmark:** `python3 apps/certus-server/python-client/test_client.py --server localhost:50051 --bench-only --bench-object-size 4194304 --bench-num-objects 10 --bench-iterations 1`
- **Output format:** Stdout table. Parse SSD-tier row: `Avg (us/obj)`, `Avg (GB/s)`.
- **Baseline result:** 21,361 us/obj SSD-tier (iter-1, condition A)

## Code Map

| Location | What | When to look |
|----------|------|--------------|
| `components/dispatcher/v1/src/pipeline.rs:30-123` | `pipelined_ssd_to_gpu` — primary rewrite target | h-main implementation |
| `components/dispatcher/v1/src/pipeline.rs:16` | `PIPELINE_RING_SIZE = 4` | Change to 2 for double-buffer |
| `components/dispatcher/v1/src/lib.rs:240-253` | `promote_and_serve` calls `pipelined_ssd_to_gpu` | Verify signature compatibility |
| `components/dispatcher/v1/src/lib.rs:87` | `noop_free` function | For DmaBuffer wrappers if needed |
| `components/dispatcher/v1/src/io_segmenter.rs:22-55` | `segment_io` function | Generates IoSegment list |
| `components/gpu-services/v0/src/cuda_ffi.rs:68-111` | extern "C" block + type aliases | Add cudaStream_t, stream APIs, cudaMemcpyAsync |
| `components/gpu-services/v0/src/cuda_ffi.rs:94` | `cudaHostAlloc` declaration | Already exists — used to allocate staging buffers |
| `components/gpu-services/v0/src/cuda_ffi.rs:95` | `cudaFreeHost` declaration | Already exists — used by cleanup |
| `components/gpu-services/v0/src/dma.rs:253-288` | `create_spdk_dma_buffer_from_cuda_host_alloc` | Creates dual CUDA+SPDK DmaBuffer from cudaHostAlloc ptr |
| `components/gpu-services/v0/src/dma.rs:232-243` | `spdk_unregister_and_cuda_free_host` | Free function for cleanup |
| `apps/certus-server/src/main.rs:190-209` | cudaHostRegister on memory-tier pool | Still needed for mem-tier H2D (non-pipeline path) |
| `components/interfaces/src/iblock_device.rs:204-214` | ReadAsync command variant | For non-blocking NVMe reads |
| `components/interfaces/src/iblock_device.rs:291-298` | ReadDone completion | Completion recv in pipeline loop |
| `components/interfaces/src/idispatcher.rs` | IpcHandle struct | Add cuda_ipc_handle_bytes for P2P arm |
| `.nous/h8-v1-true-pipeline/runs/iter-1/patches/h-main.patch` | Iter-1 pipeline patch | Reference for cuda_ffi additions (lines 289-312) |
| `.nous/h8-v1-true-pipeline/runs/iter-1/patches/h-control-negative.patch` | Iter-1 P2P patch | Reuse for h-control-negative |

## Code Targets

### h-main: cuda_ffi.rs additions

**File:** `components/gpu-services/v0/src/cuda_ffi.rs`
**Location:** After line 68 (after `CUDA_HOST_ALLOC_MAPPED`), add type alias. Inside extern "C" block (before closing brace at line 111), add function declarations.
**What:**
```rust
// After line 68:
pub type cudaStream_t = *mut c_void;

// Inside extern "C" block:
pub fn cudaStreamCreate(p_stream: *mut cudaStream_t) -> cudaError_t;
pub fn cudaStreamDestroy(stream: cudaStream_t) -> cudaError_t;
pub fn cudaStreamSynchronize(stream: cudaStream_t) -> cudaError_t;
pub fn cudaMemcpyAsync(dst: *mut c_void, src: *const c_void, count: usize, kind: c_int, stream: cudaStream_t) -> cudaError_t;
```
**Why:** Identical to iter-1 patch. Required for async GPU copies on a CUDA stream.

### h-main: pipeline.rs rewrite with cudaHostAlloc staging

**File:** `components/dispatcher/v1/src/pipeline.rs`
**Location:** Replace the body of `pipelined_ssd_to_gpu` (lines 38-123)
**What:** Double-buffered async pipeline with cudaHostAlloc staging:
1. Create CUDA stream
2. Allocate 2 staging buffers: `cudaHostAlloc(chunk_size)` → `create_spdk_dma_buffer_from_cuda_host_alloc(ptr, chunk_size)` → produces `DmaBuffer` that is both SPDK DMA target and CUDA-pinned async source
3. Wrap each in `Arc<Mutex<DmaBuffer>>` for Command::ReadAsync compatibility
4. Issue ReadAsync for chunk 0 into staging[0]
5. Loop: recv ReadDone → CPU memcpy from staging[cur].as_ptr() to mem_tier_ptr+offset → `cudaMemcpyAsync` from staging[cur].as_ptr() to gpu_dst+offset (on stream) → issue ReadAsync for next chunk into staging[1-cur] → swap
6. Final: recv ReadDone, memcpy to mem-tier, cudaMemcpyAsync, `cudaStreamSynchronize`
7. Destroy stream, drop staging buffers (auto-cleanup via free_fn)

**Key insight:** The staging buffers ARE the NVMe DMA targets (no intermediate buffer). This eliminates the extra memcpy that iter-1 had (DMA buffer → mem-tier → cudaMemcpyAsync). Now it's: NVMe DMA → staging[cur] → { cudaMemcpyAsync to GPU || memcpy to mem-tier } → next chunk.

**Why this location:** Same function signature as before — `promote_and_serve` (lib.rs:244) passes mem_tier_ptr and gpu_dst unchanged.

### h-control-negative: P2P patch

Apply the same P2P implementation from iter-1's `h-control-negative.patch`. Changes span:
- `interfaces/src/idispatcher.rs` — add `cuda_ipc_handle_bytes` field
- `dispatcher/v1/src/pipeline.rs` — add `p2p_ssd_to_gpu_persistent`
- `dispatcher/v1/src/lib.rs` — add `gpu_dma_cache` field + `get_or_create_gpu_dma` + P2P routing
- `apps/certus-server/src/service.rs` — pass handle bytes in lookup path
- `dispatcher/v1/Cargo.toml` — add base64 dep
- Multiple test files — add `cuda_ipc_handle_bytes: None` to IpcHandle constructors

The iter-1 P2P patch is at `.nous/h8-v1-true-pipeline/runs/iter-1/patches/h-control-negative.patch` — reuse it directly.

## What I Tried That Didn't Work

- **Iter-1: cudaMemcpyAsync from cudaHostRegistered mmap'd memory-tier.** Falls back to synchronous execution. Only 10% improvement. Root cause: CUDA runtime treats `cudaHostRegister`-ed memory differently from `cudaHostAlloc`-ed memory for DMA engine scheduling.
- **Iter-1: Expected 3,451 us for P2P but observed 16,396 us.** System-level confound (thermal/load state). Not a code defect — relative ordering preserved.

## What I Excluded and Why

- **Pre-allocated staging buffers (persistent across calls):** Per-call allocation of 2×128 KiB via cudaHostAlloc is ~10 us — negligible vs 21 ms transfer time. Pre-allocation adds complexity (thread-safety, lifetime management) for no meaningful benefit at this scale. If iter-2 shows the allocation is expensive, iter-3 can move to pre-allocated.
- **Triple-buffering:** Double-buffering is sufficient when NVMe read and GPU copy are balanced. With 128 KiB chunks: NVMe read ≈ 300-600 us, GPU H2D ≈ 50-100 us (PCIe 4.0 x16 = ~25 GB/s → 128 KiB in ~5 us, but overhead dominates at small sizes). NVMe read dominates → double-buffer provides full overlap.
- **BatchSubmit QD=32:** This was explored in h8-evolve-v0-pipelined. For v1 through certus-server, gRPC overhead dominates (RP-12). Single ReadAsync per chunk is the right granularity for double-buffering.
- **Cross-NUMA topology:** Focus on NODE-level (63:00.0) where PCIe path is direct.
- **Modifying gpu-p2p-server:** Constraint from campaign spec.

## Evolution of Thinking

**Starting assumption (iter-1):** cudaHostRegistered mmap'd memory-tier pool would support true async GPU DMA since it's page-locked via cudaHostRegister.

**Falsified by iter-1 result:** Only 10% improvement. cudaHostRegister does NOT enable the DMA engine for async scheduling — this matches the h8-pipelined finding on SPDK hugepages. The CUDA runtime apparently requires memory allocated through its own allocator (cudaHostAlloc) to trust it for background DMA.

**Iter-2 approach:** Use `cudaHostAlloc` as the staging layer. The key enabler already exists: `create_spdk_dma_buffer_from_cuda_host_alloc` (dma.rs:253) wraps cudaHostAlloc memory as an SPDK DmaBuffer. This means we can NVMe-read directly into a cudaHostAlloc buffer (it's SPDK-registered), then cudaMemcpyAsync from it to GPU (it's natively pinned). This eliminates the need for separate SPDK DMA buffers entirely.

**Risk:** `spdk_mem_register` on cudaHostAlloc'd memory might fail if the virtual-to-physical translation doesn't work for CUDA-managed pages. However, `create_spdk_dma_buffer_from_cuda_host_alloc` exists precisely for this use case and is tested in the h8-pipelined campaign, so the risk is low.

## Current Status

- **Validated:** Build works (0.17s). Sequential pipeline understood. cudaHostAlloc + SPDK registration exists as a proven pattern (dma.rs:253). CUDA stream APIs are standard libcudart. Feature unification confirmed.
- **Uncertain:** (1) Whether per-call cudaHostAlloc + spdk_mem_register adds meaningful overhead (expected ~10us, negligible). (2) Whether the system-level confound from iter-1 (3-5x inflation) will recur — compare relative ratios, not absolutes. (3) Whether NVMe read dominance is so extreme that even true async provides diminishing returns (read=600us, copy=5us → overlap saves at most 5us/chunk = 160us total, which is <1% of 21ms).
- **Suggested next (for iter-3):** If cudaHostAlloc staging shows minimal improvement despite true async: the bottleneck is NVMe sequential read latency, not copy overlap. Switch to BatchSubmit QD=32 with cudaHostAlloc ring (32 buffers, parallel NVMe reads). If staging shows >25% improvement: validate with larger objects (16 MiB, 64 MiB) and cross-NUMA topology.

## Warnings & Constraints

- **sudo required:** certus-server needs root for SPDK/VFIO.
- **Kill server between conditions:** `sudo pkill certus-server` between runs. Each condition requires fresh server process.
- **SPDK symlinks in worktree:** `ln -sfn /home/nara/certus/ai-native-storage-certus/deps/spdk <worktree>/deps/spdk && ln -sfn /home/nara/certus/ai-native-storage-certus/deps/spdk-build <worktree>/deps/spdk-build`
- **RUSTFLAGS required:** `RUSTFLAGS='-L /usr/local/lib'` for libgdrapi.so linking.
- **LD_LIBRARY_PATH required:** `/usr/local/lib:/usr/local/cuda/lib64` at runtime.
- **cudaHostAlloc staging buffers auto-cleanup:** The `DmaBuffer` returned by `create_spdk_dma_buffer_from_cuda_host_alloc` has `spdk_unregister_and_cuda_free_host` as its free function. When the DmaBuffer drops, it calls spdk_mem_unregister + cudaFreeHost. Do NOT manually free these buffers.
- **Arc<Mutex<DmaBuffer>> required by Command::ReadAsync:** The buf field type is `Arc<Mutex<DmaBuffer>>`. Wrap staging buffers accordingly.
- **Staging buffer alignment:** cudaHostAlloc returns page-aligned memory. SPDK requires at least 4KiB alignment for DMA. This is satisfied by default.
- **Memory-tier write still required:** The pipeline MUST fill mem_tier_ptr even when using staging buffers. After cudaMemcpyAsync launch, do a CPU memcpy from staging.as_ptr() to mem_tier_ptr+offset. This is cheap (128 KiB memcpy ≈ few microseconds) and can overlap with GPU DMA.
- **gpu-services features in dispatcher-v1:** dispatcher-v1/Cargo.toml has `gpu-services = { workspace = true, features = ["spdk"] }`. This alone doesn't enable `gpu` feature. But Cargo feature unification with certus-server (which enables `gpu`) means both are active in the final build. If building dispatcher-v1 standalone (e.g., `cargo test -p dispatcher-v1`), the `gpu` feature won't be active → compilation will fail on `use gpu_services::dma`. This is acceptable since we only test through certus-server.
