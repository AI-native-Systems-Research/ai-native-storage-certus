# Handoff — h8-pipelined Iteration 2

## Goal

Implement a pipelined bounce-buffer transfer mode using **CUDA-native pinned memory** (`cudaHostAlloc` + SPDK registration) and measure whether it achieves true async overlap, outperforming non-pipelined bounce for 4 MiB / 32×128 KiB chunk transfers.

## Key Discoveries

- **`create_spdk_dma_buffer_from_cuda_host_alloc` already exists at `dma.rs:253`.** It takes a `cudaHostAlloc` pointer, calls `spdk_mem_register`, and wraps it as a `DmaBuffer` with proper cleanup (`spdk_unregister_and_cuda_free_host` at `dma.rs:232`). This is the key enabler — no new DMA infrastructure needed.
- **Iter-1 failure root cause: per-request `cudaHostRegister` + SPDK hugepage buffers.** The iter-1 patch (`patches/h-main.patch`) registered SPDK hugepage buffers with CUDA per-request. This (a) doesn't enable true async (CUDA treats registered hugepages differently than natively-pinned memory), and (b) adds ~200μs per-transfer overhead. Pipeline was 11-13% slower than non-pipelined (2.07ms vs 1.87ms).
- **Pre-allocation eliminates per-request overhead.** Buffers can be allocated once at startup (before the accept loop) and reused across all client requests. The CUDA stream should also be created once. This makes the per-request overhead negligible.
- **`DmaBuffer::from_raw` at `spdk_types.rs:293` supports custom deallocators.** The existing `create_spdk_dma_buffer_from_cuda_host_alloc` uses this with `spdk_unregister_and_cuda_free_host` as the free function — handles both SPDK unregister and CUDA free on drop.
- **Feature gates are correct.** The p2p feature enables both `gpu` and `spdk` (Cargo.toml:11), so `create_spdk_dma_buffer_from_cuda_host_alloc` (gated `#[cfg(all(feature = "gpu", feature = "spdk"))]`) is available in the p2p_server binary.
- **Build validated.** `RUSTFLAGS="-L /usr/local/lib" cargo build -p gpu-services --features p2p` exits 0 on the current `hypothesis_8` branch.
- **Iter-1 cuda_ffi.rs additions are NOT on this branch.** The iter-1 patch was applied in a worktree that was cleaned up. The executor must re-apply the FFI additions (cudaStream_t, cudaStreamCreate, etc.) from iter-1's `h-main.patch`.

## System Interface

- **Build:** `RUSTFLAGS="-L /usr/local/lib" cargo build -p gpu-services --features p2p`
- **Run baseline:** `sudo target/debug/gpu-p2p-server --mode bounce --chunk-size 131072` + `python3 components/gpu-services/v0/tests/gpu_client_p2p.py 4194304 /tmp/gpu_p2p_server.sock --iterations 10`
- **Output format:** Client reports to stderr: `Throughput: X.X MB/s`, `Avg latency: X.XX ms`, `Min latency: X.XX ms`, `Max latency: X.XX ms`
- **Baseline result:** Bounce 1.87ms / 2143 MB/s; P2P warm 1.27-1.34ms / 2976-3147 MB/s (iter-1, same hardware)

## Code Map

| Location | What | When to look |
|----------|------|--------------|
| `p2p_server.rs:28-36` | `TransferMode` enum | Adding `BouncePipelineV2` variant |
| `p2p_server.rs:38-64` | CLI struct (Clap) | Adding `bounce-pipeline-v2` CLI value |
| `p2p_server.rs:374-433` | `handle_bounce` | Reference for non-pipelined flow |
| `p2p_server.rs:272-323` | `do_chunked_read` (BatchSubmit) | Understanding the individual ReadAsync structure within BatchSubmit |
| `p2p_server.rs:569-604` | `main()` mode match for chunk pool | Adding pre-allocation of pipeline buffers (similar pattern to P2p chunk pool) |
| `p2p_server.rs:640-654` | `main()` request dispatch | Adding pipeline-v2 handler call |
| `cuda_ffi.rs:71-111` | FFI extern block | Adding stream/async declarations (re-apply from iter-1 patch) |
| `cuda_ffi.rs:94` | `cudaHostAlloc` declaration | Already exists — use for buffer allocation |
| `cuda_ffi.rs:95` | `cudaFreeHost` declaration | Already exists — used by cleanup |
| `dma.rs:253-288` | `create_spdk_dma_buffer_from_cuda_host_alloc` | Wraps cudaHostAlloc ptr as SPDK DmaBuffer |
| `dma.rs:232-243` | `spdk_unregister_and_cuda_free_host` | Free function — understand cleanup path |
| `interfaces/src/iblock_device.rs:205-214` | `ReadAsync` command | Individual async reads for pipeline |
| `interfaces/src/iblock_device.rs:293-297` | `ReadDone` completion | Completion recv in pipeline loop |
| `interfaces/src/spdk_types.rs:293-314` | `DmaBuffer::from_raw` | How external memory becomes DmaBuffer |

## Code Targets

### h-main arm — cuda_ffi.rs additions (re-apply from iter-1)

**File:** `components/gpu-services/v0/src/cuda_ffi.rs`
**Location:** Inside the extern "C" block (line 71-111), add before closing brace
**Change:** Add these declarations (identical to iter-1's patch):
```rust
pub type cudaStream_t = *mut c_void;
pub const CUDA_HOST_REGISTER_DEFAULT: c_int = 0;

// Inside extern "C" block:
pub fn cudaStreamCreate(p_stream: *mut cudaStream_t) -> cudaError_t;
pub fn cudaStreamDestroy(stream: cudaStream_t) -> cudaError_t;
pub fn cudaStreamSynchronize(stream: cudaStream_t) -> cudaError_t;
pub fn cudaMemcpyAsync(dst: *mut c_void, src: *const c_void, count: usize, kind: c_int, stream: cudaStream_t) -> cudaError_t;
```
**Why:** These were in iter-1's patch but the worktree was cleaned up. Same symbols in libcudart.so.

### h-main arm — p2p_server.rs pipelined handler v2

**File:** `components/gpu-services/v0/src/bin/p2p_server.rs`

**Change 1:** Add `BouncePipelineV2` to `TransferMode` enum (line 28-36).

**Change 2:** In `main()`, at the same location as the P2p chunk pool pre-allocation (line 582-604), add startup allocation for the pipeline-v2 mode:
```
- Allocate 2 buffers via cudaHostAlloc (each chunk_size bytes)
- Wrap each via gpu_services::dma::create_spdk_dma_buffer_from_cuda_host_alloc
- Create one CUDA stream via cudaStreamCreate
- Store these in a struct (e.g., PipelineState { bufs: [Arc<Mutex<DmaBuffer>>; 2], stream: cudaStream_t })
```

**Change 3:** After `handle_bounce` (line 433), add `handle_bounce_pipeline_v2` function:
```
fn handle_bounce_pipeline_v2(
    stream: &mut UnixStream,
    ctx: &ServerContext,
    chunk_size: usize,
    pipeline: &PipelineState,  // pre-allocated buffers + CUDA stream
) -> Result<String, String>
```
The function:
1. Parse client payload, open IPC handle
2. Connect block device client (get channels)
3. Submit ReadAsync for chunk 0 into pipeline.bufs[0]
4. Loop: recv ReadDone for bufs[current], launch cudaMemcpyAsync H2D, submit ReadAsync for next chunk into bufs[1-current], swap
5. After last ReadDone + last cudaMemcpyAsync → cudaStreamSynchronize
6. Close IPC handle, return

**Critical difference from iter-1:** No per-request `cudaHostRegister`/`cudaHostUnregister`. No per-request stream creation. Buffers and stream are pre-allocated and passed in.

**Change 4:** Wire in main() dispatch match (line 642) and mode_str match.

**Import needed:** `use gpu_services::dma::create_spdk_dma_buffer_from_cuda_host_alloc;`

## What I Tried That Didn't Work

- **Iter-1: SPDK hugepage buffers + per-request `cudaHostRegister`** — pipeline was 11-13% SLOWER than non-pipelined. Root cause: cudaMemcpyAsync falls back to synchronous with these buffers, plus registration overhead dominates.
- **Looking for `cudaStreamQuery` in FFI** — not present. Not needed for this iter; could add for diagnostic in iter-3 if results are ambiguous.

## What I Excluded and Why

- **Triple-buffering:** Double-buffering is the minimum needed to prove async overlap works. If the NVMe and H2D phases are balanced (~58μs each), double-buffering achieves maximum overlap. Triple-buffering only helps if one phase is significantly faster than the other (buffer starved). Try in iter-3 if double-buffer shows partial but not full overlap.
- **Larger chunk sizes:** MDTS constraint (128 KiB). Would reduce per-chunk overhead but changes a fundamental parameter. Separate experiment.
- **Non-blocking NVMe completion (`try_recv`):** Would enable fully async pipeline (CPU never blocks), but requires interface changes. The blocking recv in double-buffer mode still achieves overlap because cudaMemcpyAsync returns immediately to CPU. Overkill for this experiment.
- **`cudaStreamQuery` diagnostic:** Would verify async execution at runtime. Not needed for the experiment — if latency drops meaningfully, async is working. Add in iter-3 if results are ambiguous.
- **Modifying the existing `BouncePipeline` mode:** Creating `BouncePipelineV2` as a separate mode allows direct A/B comparison with iter-1's approach if needed for debugging. Clean separation.

## Evolution of Thinking

Iter-1 designed the pipeline assuming `cudaHostRegister` on SPDK hugepages would work for async. It didn't — the key insight from RP-4 is that CUDA needs to manage the memory from birth (`cudaHostAlloc`) for its DMA engine to use it without internal staging.

The critical discovery in this exploration: `create_spdk_dma_buffer_from_cuda_host_alloc` already exists in the codebase (dma.rs:253). It was built for exactly this use case — making `cudaHostAlloc` memory available for NVMe DMA. This eliminates any concern about whether SPDK can DMA into non-hugepage pinned memory (it can, via `spdk_mem_register`).

The second insight: pre-allocation eliminates the per-request overhead that dominated iter-1. Iter-1's pipeline called `cudaHostRegister` + `cudaHostUnregister` + `cudaStreamCreate` + `cudaStreamDestroy` per request. With startup allocation, per-request overhead is just: open IPC handle + ReadAsync commands + cudaMemcpyAsync launches + cudaStreamSynchronize.

## Current Status

- **Validated:** Build works. `create_spdk_dma_buffer_from_cuda_host_alloc` exists and is accessible. Feature gates are correct. Iter-1's failure mechanism is understood.
- **Uncertain:** Whether `spdk_mem_register` succeeds on `cudaHostAlloc` memory (it should — the function exists for this purpose and was committed, but hasn't been tested in the pipeline context). If it fails, the error will be clear (`rc != 0`).
- **Suggested next (for iter-3):** If iter-2 shows speedup but not reaching P2P levels: (a) add `cudaStreamQuery` to verify true async at runtime, (b) try triple-buffering if NVMe/H2D imbalance detected, (c) profile with nsys to measure actual overlap percentage. If iter-2 shows no speedup despite cudaHostAlloc: the PCIe topology may share a root complex between NVMe and GPU (contention, not true independence) — would need to verify with `lspci -tv`.

## Warnings & Constraints

- **sudo required:** Server needs root for SPDK/VFIO.
- **Do NOT use `--once` for benchmark mode:** Client opens new socket per iteration (gpu_client_p2p.py:46-61). Server must stay running.
- **RUSTFLAGS required:** `RUSTFLAGS="-L /usr/local/lib"` to find libgdrapi.so.
- **Iter-1 patch NOT on branch:** The cuda_ffi.rs additions (cudaStream_t, cudaMemcpyAsync, etc.) must be re-implemented. The iter-1 worktree was cleaned up.
- **Buffer ownership:** The pre-allocated DmaBuffers must NOT be dropped between requests. Store them in an Arc<Mutex<>> at the main() level and pass references to the handler. The DmaBuffer's Drop calls `spdk_unregister_and_cuda_free_host` — only drop at server shutdown.
- **CUDA stream thread safety:** CUDA streams are not thread-safe, but the server is single-threaded (sequential accept loop at p2p_server.rs:634-674). Safe to reuse stream across requests.
- **Import path:** `use gpu_services::dma::create_spdk_dma_buffer_from_cuda_host_alloc;` — the function is pub in a pub mod.
