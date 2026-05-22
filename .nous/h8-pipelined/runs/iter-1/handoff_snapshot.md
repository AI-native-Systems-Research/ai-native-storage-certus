# Handoff — h8-pipelined Iteration 1

## Goal

Implement a pipelined bounce-buffer transfer mode (`--mode bounce-pipeline`) in the GPU P2P server and measure whether overlapping NVMe DMA with cudaMemcpyAsync H2D achieves latency competitive with direct SSD→GPU P2P.

## Key Discoveries

- **No pipelining exists.** Current `handle_bounce` (p2p_server.rs:374-433) is strictly two-phase: BatchSubmit all 32 reads → wait all completions → 32 sequential cudaMemcpy H2D. Zero overlap.
- **Prior measurements prove pipelining is viable.** NVMe read = 790μs, H2D copy = 819μs (nearly equal). These use independent PCIe hardware. Theoretical pipelined time: max(790,819) ≈ 820μs vs serial 1610μs.
- **CUDA async APIs missing from FFI.** `cuda_ffi.rs` has only synchronous `cudaMemcpy`. Must add: `cudaMemcpyAsync`, `cudaStreamCreate`, `cudaStreamSynchronize`, `cudaStreamDestroy`. All in libcudart.so (already linked).
- **Individual ReadAsync completions work.** `BatchSubmit` dispatches N `ReadAsync` ops and produces N individual `ReadDone` completions (iblock_device.rs:236,293). For pipelining, submit reads one-at-a-time (not BatchSubmit) to get early completions.
- **Host DMA buffers come from SPDK hugepages.** `interfaces::DmaBuffer::new(size, sector_size, None)` allocates from SPDK. These are CPU-accessible and NVMe-DMAable. For `cudaMemcpyAsync` to work at full speed, consider using `cudaHostRegister` on the SPDK buffer to pin it in CUDA's address space (pageable memory uses a staging path internally).
- **P2P warm = 1.32ms, bounce = 2.65ms** (4 MiB, 128 KiB chunks, 10 iterations). Build validated: `RUSTFLAGS="-L /usr/local/lib" cargo build -p gpu-services --features p2p` exits 0.

## System Interface

- **Build:** `RUSTFLAGS="-L /usr/local/lib" cargo build -p gpu-services --features p2p`
- **Run baseline:** `sudo target/debug/gpu-p2p-server --mode bounce --chunk-size 131072 --once` + `python3 components/gpu-services/v0/tests/gpu_client_p2p.py 4194304 /tmp/gpu_p2p_server.sock --iterations 10`
- **Output format:** Client reports to stderr: `Avg latency: X.XX ms`, `Throughput: Y.Y MB/s` (benchmark mode)
- **Baseline result:** Bounce 2.65ms / 1510 MB/s; P2P warm 1.32ms / 3031 MB/s (prior experiment, same hardware)

## Code Map

| Location | What | When to look |
|----------|------|--------------|
| `p2p_server.rs:28-36` | `TransferMode` enum | Adding `BouncePipeline` variant |
| `p2p_server.rs:38-64` | CLI struct (Clap) | Adding `bounce-pipeline` CLI value |
| `p2p_server.rs:374-433` | `handle_bounce` | Reference for non-pipelined flow |
| `p2p_server.rs:272-323` | `do_chunked_read` (BatchSubmit) | Understanding multi-read dispatch (NOT used in pipeline — use individual ReadAsync instead) |
| `p2p_server.rs:582-604` | `main` mode match for chunk pool | Adding pipeline mode branch |
| `p2p_server.rs:640-654` | `main` request dispatch | Adding pipeline handler call |
| `cuda_ffi.rs:71-111` | FFI extern block | Adding stream/async declarations |
| `interfaces/src/iblock_device.rs:205-214` | `ReadAsync` command | Individual async reads for pipeline |
| `interfaces/src/iblock_device.rs:293-297` | `ReadDone` completion | Completion structure for recv loop |
| `Cargo.toml:1-45` | gpu-services crate config | No changes needed — libcudart already linked |

## Code Targets

### h-main arm — cuda_ffi.rs additions

**File:** `components/gpu-services/v0/src/cuda_ffi.rs`
**Location:** After line 111 (end of extern block), or inside the existing extern block at line 71-111
**Change:** Add 4 FFI declarations:
```
pub type cudaStream_t = *mut c_void;
pub fn cudaStreamCreate(pStream: *mut cudaStream_t) -> cudaError_t;
pub fn cudaStreamDestroy(stream: cudaStream_t) -> cudaError_t;
pub fn cudaStreamSynchronize(stream: cudaStream_t) -> cudaError_t;
pub fn cudaMemcpyAsync(dst: *mut c_void, src: *const c_void, count: usize, kind: c_int, stream: cudaStream_t) -> cudaError_t;
```
**Why this location:** All CUDA FFI is in this one file. The existing extern "C" block (line 71) links against libcudart.so. These functions are all in the same library.

### h-main arm — p2p_server.rs pipelined handler

**File:** `components/gpu-services/v0/src/bin/p2p_server.rs`
**Location:** After `handle_bounce` (line 433), add `handle_bounce_pipeline` function
**Change:** New ~60-line function implementing:
1. Allocate 2 host DMA buffers (double-buffer)
2. Open client IPC handle (reuse `open_ipc_handle`)
3. Create CUDA stream
4. Pipeline loop: submit ReadAsync → recv ReadDone → cudaMemcpyAsync → submit next ReadAsync → swap buffers
5. Final cudaStreamSynchronize
6. Cleanup

**Why individual ReadAsync (not BatchSubmit):** BatchSubmit fires all reads simultaneously and returns completions as they finish — but we need to wait for one at a time so we can kick off the copy immediately. Individual ReadAsync per chunk gives us one-completion-at-a-time control flow.

Also: add `BouncePipeline` to `TransferMode` enum and wire it in the match at line 642.

## What I Tried That Didn't Work

- Grepped for any existing pipelining, double-buffering, or async stream code — nothing found
- Checked if `cudaMemcpyAsync`/`cudaStreamCreate` were already in FFI — they are not
- Prior hypothesis-8 experiment (different campaign) ran for 2 iterations without ever implementing pipelining; the analysis document confirms this was the critical gap

## What I Excluded and Why

- **`cudaHostAlloc` for pipeline buffers:** Could improve H2D bandwidth by using CUDA-pinned host memory instead of SPDK hugepage buffers. Excluded from iter-1 because SPDK DMA buffers are required for NVMe reads, and `cudaHostRegister` on existing buffers is simpler than managing separate allocations. If pipeline doesn't achieve expected speedup, try this in iter-2.
- **Larger chunk sizes:** MDTS constraint (128 KiB default). Could experiment with larger chunks to reduce per-chunk overhead, but this changes a fundamental parameter and should be a separate experiment.
- **More than 2 pipeline buffers (triple-buffering):** Two buffers should be sufficient given that read time (24.7μs/chunk) and copy time (25.6μs/chunk) are nearly equal. Triple-buffering adds complexity without benefit when stages are balanced. Revisit if measurements show imbalance.
- **Async NVMe + async H2D (fully asynchronous):** Would require a non-blocking NVMe completion check (`try_recv` instead of `recv`). The current channel-based interface blocks on recv. Double-buffering with blocking recv still achieves overlap because the *previous* chunk's async copy runs while we block waiting for the *current* chunk's read.

## Evolution of Thinking

Started expecting a straightforward `cudaMemcpyAsync` drop-in. Reading the code revealed:
1. The NVMe interface uses channels (blocking `recv`) — true async would need redesign
2. But double-buffering with blocking recv still works: while blocked on recv(chunk[i+1]), the async copy of chunk[i] runs on the GPU
3. The key insight is that `cudaMemcpyAsync` returns immediately to the CPU, so the CPU can then block on the next NVMe completion while the GPU copy engine works in parallel
4. Prior experiment data (790μs read, 819μs copy, nearly equal) confirms this is the ideal pipelining scenario

## Current Status

- **Validated:** Build command works. Prior experiment data confirms viability. No existing pipelining code to conflict with.
- **Uncertain:** Whether SPDK hugepage buffers work well with `cudaMemcpyAsync` (they're physically contiguous but not CUDA-registered). May need `cudaHostRegister` call on the DMA buffer pointer for full async bandwidth.
- **Suggested next:** If iter-1 shows pipelining works but doesn't quite match P2P, iter-2 should: (a) try `cudaHostRegister` on SPDK buffers, (b) profile with nsys/nvprof to verify true overlap, (c) test triple-buffering if there's buffer-swap latency.

## Warnings & Constraints

- **sudo required:** The server needs root for SPDK/VFIO. The client runs as normal user.
- **`--once` flag critical for benchmarking:** Without it, the server stays running and the client must reconnect per iteration (which it does). With `--once`, server exits after one client session — use only for single-iteration validation. For `--iterations 10` benchmark mode, do NOT use `--once`.
- **Actually, re-reading the client:** Each iteration in benchmark mode opens a NEW socket connection (`do_transfer` creates a fresh socket per call, line 44-61). So the server must NOT use `--once` when the client uses `--iterations N > 1`. Remove `--once` for benchmark runs.
- **RUSTFLAGS required:** The build needs `RUSTFLAGS="-L /usr/local/lib"` to find libgdrapi.so. Without it, linking fails.
- **cudaMemcpyAsync with non-pinned host memory:** If the host buffer is not page-locked (via cudaHostAlloc or cudaHostRegister), cudaMemcpyAsync will silently fall back to synchronous behavior. The SPDK hugepage buffers ARE physically pinned (huge pages are always resident) but CUDA doesn't know that. Executor should add `cudaHostRegister` on the DMA buffer pointer after allocation.
