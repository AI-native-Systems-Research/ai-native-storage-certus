# Handoff — h8-v1-true-pipeline Iteration 1

## Goal

Implement true overlapped pipelining in dispatcher v1's `pipelined_ssd_to_gpu` (double-buffered ReadAsync + cudaMemcpyAsync) and compare its SSD-tier lookup latency against direct P2P DMA, all through certus-server with 4 MiB objects on NVMe 63:00.0.

## Key Discoveries

- **The existing pipeline is fully sequential** (pipeline.rs:60-119). Each chunk: send ReadSync → recv ReadDone → memcpy to mem-tier → sync cudaMemcpy H2D. Ring of 4 buffers allocated but only used round-robin, never concurrently.
- **Memory-tier pool IS cudaHostRegistered** (main.rs:191-209). The 256 MiB mmap'd pool is registered at server startup. This makes it a valid source for `cudaMemcpyAsync` — the DMA engine can access it without internal staging. HOWEVER: h8-pipelined found that `cudaHostRegister` on SPDK hugepages does NOT enable true async. The memory-tier pool is mmap'd (not hugepages), so the behavior may differ. This is the key uncertainty.
- **ReadAsync command exists** (interfaces/src/iblock_device.rs:204-214). Takes the same params as ReadSync plus a timeout_ms. Returns ReadDone completion like ReadSync. This enables non-blocking NVMe reads.
- **No cudaMemcpyAsync/cudaStream in cuda_ffi.rs** — must be added. These are standard libcudart symbols; same pattern as the existing cudaMemcpy binding.
- **P2P baseline from h8-v1-pinned iter-2:** bounce=7,029 us/obj, P2P=3,451 us/obj (both NODE-level, NVMe 63:00.0, 4 MiB, 10 objects, 1 iteration).
- **32 chunks per 4 MiB object** (128 KiB MDTS, confirmed in io_segmenter tests at io_segmenter.rs:112-120).
- **Build validated:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server` works on `hypothesis_8` branch (from h8-v1-pinned iter-2 handoff).

## System Interface

- **Build:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server`
- **Run baseline:** `LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH sudo target/debug/certus-server --metadata-pci 0000:62:00.0 --data-pci 0000:63:00.0 --dispatcher-version v1 --listen 0.0.0.0:50051` then `python3 apps/certus-server/python-client/test_client.py --server localhost:50051 --bench-only --bench-object-size 4194304 --bench-num-objects 10 --bench-iterations 1`
- **Output format:** Table on stdout with SSD-tier row: Avg (us/obj), Min, Max, Avg GB/s, Peak GB/s
- **Baseline result:** 7,029 us/obj SSD-tier (h8-v1-pinned iter-2, condition-A)

## Code Map

| Location | What | When to look |
|----------|------|--------------|
| `components/dispatcher/v1/src/pipeline.rs:30-123` | `pipelined_ssd_to_gpu` — the function to rewrite | Primary target for h-main |
| `components/dispatcher/v1/src/pipeline.rs:14` | `PIPELINE_RING_SIZE = 4` | Change to 2 for double-buffer |
| `components/dispatcher/v1/src/lib.rs:240-253` | `promote_and_serve` calls `pipelined_ssd_to_gpu` | Verify function signature compatibility |
| `components/dispatcher/v1/src/io_segmenter.rs:22-55` | `segment_io` function | Generates IoSegment list for chunked I/O |
| `components/gpu-services/v0/src/cuda_ffi.rs:71-111` | extern "C" block | Add cudaStream_t, cudaStreamCreate, cudaMemcpyAsync here |
| `components/gpu-services/v0/src/cuda_ffi.rs:63-65` | CUDA_MEMCPY direction constants | Use CUDA_MEMCPY_HOST_TO_DEVICE (=1) for H2D async copy |
| `apps/certus-server/src/main.rs:191-209` | cudaHostRegister on memory-tier pool | Confirms mem-tier is CUDA-pinned |
| `apps/certus-server/src/main.rs:174-258` | v1 dispatcher initialization | Server startup, component wiring |
| `apps/certus-server/src/service.rs:179-254` | gRPC lookup handler | Where IPC handle flows to dispatcher.lookup |
| `components/interfaces/src/iblock_device.rs:204-214` | ReadAsync command variant | For non-blocking NVMe reads |
| `components/interfaces/src/iblock_device.rs:291-298` | ReadDone completion | Completion message structure |
| `components/interfaces/src/idispatcher.rs:108-119` | IpcHandle struct | Add cuda_ipc_handle_bytes for P2P arm |
| `components/dispatcher/v1/Cargo.toml` | Crate dependencies | Add gpu-services, base64 |

## Code Targets

### h-main: cuda_ffi.rs additions

**File:** `components/gpu-services/v0/src/cuda_ffi.rs`
**Location:** Before the closing brace of the extern "C" block (line 111), and a type alias before the block.
**What:** Add:
```rust
pub type cudaStream_t = *mut c_void;
// Inside extern "C":
pub fn cudaStreamCreate(p_stream: *mut cudaStream_t) -> cudaError_t;
pub fn cudaStreamDestroy(stream: cudaStream_t) -> cudaError_t;
pub fn cudaStreamSynchronize(stream: cudaStream_t) -> cudaError_t;
pub fn cudaMemcpyAsync(dst: *mut c_void, src: *const c_void, count: usize, kind: c_int, stream: cudaStream_t) -> cudaError_t;
```
**Why:** Standard libcudart symbols, same linking as existing cudaMemcpy.

### h-main: pipeline.rs rewrite

**File:** `components/dispatcher/v1/src/pipeline.rs`
**Location:** Replace the body of `pipelined_ssd_to_gpu` (lines 38-123)
**What:** Double-buffered async pipeline:
1. Create CUDA stream
2. Allocate 2 DMA ring buffers
3. Issue ReadAsync for chunk 0 into buf[0]
4. Loop: recv ReadDone for buf[cur], memcpy to mem-tier, cudaMemcpyAsync H2D from mem-tier offset to GPU offset, issue ReadAsync for next chunk into buf[1-cur], swap cur
5. Final: recv ReadDone, memcpy to mem-tier, cudaMemcpyAsync, cudaStreamSynchronize
6. Destroy stream

**Why:** The function signature stays the same (`mem_tier_ptr`, `gpu_dst` params unchanged). The behavior changes from sequential to overlapped. The caller (`promote_and_serve`) is unaffected.

### h-control-negative: P2P patch

Apply the existing P2P patch pattern from h8-v1-pinned iter-2. Changes span:
- `interfaces/src/idispatcher.rs` — add `cuda_ipc_handle_bytes` field
- `dispatcher/v1/src/pipeline.rs` — add `p2p_ssd_to_gpu_persistent`
- `dispatcher/v1/src/lib.rs` — add `gpu_dma_cache` field + `get_or_create_gpu_dma` + P2P routing in `promote_and_serve`
- `apps/certus-server/src/service.rs` — pass handle bytes in lookup path
- `dispatcher/v1/Cargo.toml` — add base64 dep
- Various test files — add `cuda_ipc_handle_bytes: None` to IpcHandle constructors

## What I Tried That Didn't Work

- N/A (first iteration of this campaign).

## What I Excluded and Why

- **Triple-buffering or larger ring:** Double-buffering is sufficient if NVMe read and GPU copy are balanced. If iter-1 shows partial but not full overlap, escalate to triple-buffer in iter-2.
- **cudaHostAlloc ring buffers instead of memory-tier:** The h8-pipelined experiment showed that cudaHostAlloc enables true async, while cudaHostRegister on SPDK hugepages does NOT. The memory-tier pool is mmap'd + cudaHostRegistered — it's unclear whether this behaves like hugepages (no async) or like regular mmap (async works). We test with memory-tier first since it's the natural path. If it fails, iter-2 should try cudaHostAlloc ring buffers.
- **Modifying the IDispatcher or IBlockDevice interfaces:** The existing Command::ReadAsync and the pipeline function signature are sufficient. No interface changes needed for h-main.
- **Dispatcher v0:** Out of scope — v1 is the memory-tier variant we're studying.
- **Cross-NUMA topology (c2:00.0):** Focus on NODE-level first (63:00.0) where PCIe is optimal. Cross-NUMA adds Infinity Fabric complexity — test in iter-2 if needed.

## Evolution of Thinking

Starting assumption: the "pipeline" in v1 is just a marketing name — it's fully sequential. Confirmed by reading pipeline.rs:60-119 where each chunk does ReadSync (blocking) → memcpy → sync cudaMemcpy before advancing to the next.

Key insight from h8-pipelined handoff: cudaHostRegister on SPDK hugepages doesn't enable async. This raises the question: does it work on mmap'd memory (which is what memory-tier uses)? The CUDA documentation says cudaHostRegister should enable async on any page-locked memory, but the h8-pipelined result suggests implementation reality differs from documentation. This is the primary risk for h-main.

Mitigation: even if cudaMemcpyAsync falls back to sync, the ReadAsync should still provide SOME overlap (NVMe read overlaps with the sync cudaMemcpy + memcpy of the previous chunk). The overlap is between the NVMe DMA engine and the CPU/GPU copy, not between two GPU DMA transfers. This should still yield improvement.

## Current Status

- **Validated:** Build works. Sequential pipeline understood. P2P comparison numbers available. ReadAsync command exists. CUDA stream APIs are standard libcudart.
- **Uncertain:** Whether cudaMemcpyAsync from cudaHostRegistered mmap'd memory actually runs asynchronously (the key risk). If it does, expect ~1.5-2x speedup. If it falls back to sync, expect modest improvement from NVMe/memcpy overlap only.
- **Suggested next (for iter-2):** If cudaMemcpyAsync falls back to sync: try cudaHostAlloc ring buffers (proven to work in h8-pipelined). If pipeline works but is slower than P2P: measure per-stage timing to identify the bottleneck (is it the memcpy to memory-tier? the GPU copy? or NVMe read dominance?).

## Warnings & Constraints

- **sudo required:** certus-server needs root for SPDK/VFIO.
- **Kill server between conditions:** Each condition requires a fresh server process. Use `sudo pkill certus-server` between runs.
- **SPDK symlinks in worktree:** If running in a worktree, symlink `deps/spdk` and `deps/spdk-build` from the main repo (they're gitignored and not copied). Command: `ln -sfn /home/nara/certus/ai-native-storage-certus/deps/spdk <worktree>/deps/spdk && ln -sfn /home/nara/certus/ai-native-storage-certus/deps/spdk-build <worktree>/deps/spdk-build`
- **RUSTFLAGS required:** `RUSTFLAGS='-L /usr/local/lib'` for libgdrapi.so linking.
- **LD_LIBRARY_PATH required:** `/usr/local/lib:/usr/local/cuda/lib64` at runtime for libcudart.so and libgdrapi.so.
- **Wait for write-through:** The benchmark client already waits 3s after populate for background write-through. But with only 10 objects beyond pool capacity, verify that all 10 have ssd_offset set before the cold lookup (otherwise promote_and_serve won't trigger).
- **Memory-tier only has 256 MiB:** With 4 MiB objects, pool holds 64. Benchmark populates 74 (64+10). The first 10 get evicted and must have write-through complete for SSD-tier lookup to work.
- **P2P arm modifies IpcHandle struct:** This requires updating ALL IpcHandle constructors across the workspace (tests, benchmarks, other dispatchers). The h8-v1-pinned patch shows all locations — follow that pattern.
- **gpu-services dependency for pipeline.rs:** Adding `gpu-services` as a dep to dispatcher-v1 Cargo.toml may create a dependency cycle. Check the workspace graph. If it does, extract just the cuda_ffi module into a separate small crate, or use raw extern "C" declarations directly in pipeline.rs (uglier but avoids the cycle).
