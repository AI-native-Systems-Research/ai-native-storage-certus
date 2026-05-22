# Handoff: Pipelined Bounce Buffer Transfer (Iteration 1)

## Goal

Implement true pipelining in bounce mode (overlap NVMe reads with cudaMemcpyAsync H2D copies using double-buffering) and measure whether it closes the 2x latency gap with P2P warm mode. Prior iterations proved the gap exists and decomposed it into phases; this iteration implements the optimization the research question asks about.

## Key Discoveries

- **No pipelining exists in the code.** `handle_bounce` (p2p_server.rs:374-433) does BatchSubmit of all 32 NVMe reads, waits for all completions, then runs 32 sequential cudaMemcpy H2D copies. This is read-all-then-copy-all, not a pipeline.
- **No cudaMemcpyAsync or CUDA streams in the FFI.** `cuda_ffi.rs` only has synchronous `cudaMemcpy`. Must add: `cudaMemcpyAsync`, `cudaStreamCreate`, `cudaStreamSynchronize`, `cudaStreamDestroy`, `cudaStream_t`.
- **NVMe reads and H2D copies use independent hardware with no interference.** Iter-2 ablation confirmed <4% timing difference with/without NVMe traffic during H2D copies. This means pipelining should achieve near-perfect overlap.
- **Per-chunk timing is balanced for pipelining.** Bounce: read=790μs/32=24.7μs per chunk, copy=819μs/32=25.6μs per chunk. Nearly equal ⇒ neither phase starves the other, pipeline stays full.
- **SPDK DMA buffers are hugepage-backed.** `DmaBuffer::new` allocates from SPDK hugepage pool (pinned physical memory). CUDA requires pinned host memory for truly async `cudaMemcpyAsync` — verify SPDK buffers satisfy this requirement via `cudaHostRegister` or check if they're already page-locked.
- **Individual ReadAsync returns per-chunk completions.** Unlike BatchSubmit (which dispatches all at once), sending individual `ReadAsync` commands allows receiving completions one-by-one for pipeline progression. Both go through the same NVMe submission queue.
- **Build validated:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server` exits 0.

## System Interface

- **Build:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server`
- **Run baseline:**
  ```bash
  bash .nous/h8-transfer-path/runs/iter-1/inputs/run_condition.sh \
    bounce results/sequential-bounce.txt 0000:62:00.0
  ```
- **Output format:** Client (`gpu_client_p2p.py`) reports to stderr: Throughput (MB/s), Avg/Min/Max latency (ms). Server response per request: `OK <size> bytes (<mode>, <chunks> chunks) [read_us=N copy_us=N total_us=N]`. Run script captures combined output.
- **Baseline result:** Bounce 1510-1544 MB/s, 2.59-2.65ms avg latency. P2P warm 3031-3064 MB/s, 1.31-1.32ms.
- **Runtime env:** `LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64`

## Code Map

| Location | What | When to look |
|----------|------|-------------|
| `components/gpu-services/v0/src/bin/p2p_server.rs:28-36` | `TransferMode` enum | Adding `BouncePipelined` variant |
| `components/gpu-services/v0/src/bin/p2p_server.rs:38-64` | `Cli` struct | Adding new mode to `value_enum` |
| `components/gpu-services/v0/src/bin/p2p_server.rs:272-322` | `do_chunked_read` | Understanding batch NVMe reads (don't use for pipelined — use individual ReadAsync) |
| `components/gpu-services/v0/src/bin/p2p_server.rs:374-433` | `handle_bounce` | Template for pipelined version; add timing instrumentation |
| `components/gpu-services/v0/src/bin/p2p_server.rs:436-490` | `handle_p2p` | Add timing instrumentation for comparison |
| `components/gpu-services/v0/src/bin/p2p_server.rs:569-678` | `main()` match on cli.mode | Routing new mode to handler |
| `components/gpu-services/v0/src/cuda_ffi.rs:71-111` | CUDA FFI extern block | Adding async copy and stream functions |
| `components/gpu-services/v0/tests/gpu_client_p2p.py:44-61` | `do_transfer` | Parsing phase timing from response |
| `components/gpu-services/v0/tests/gpu_client_p2p.py:116-151` | Benchmark reporting | Adding phase breakdown columns |
| `components/interfaces/src/iblock_device.rs:204-214` | `ReadAsync` command | Individual async read (needed for per-chunk pipeline) |
| `components/block-device-spdk-nvme/v1/src/actor.rs:467-485` | ReadAsync dispatch | Sends completion per-op, enabling per-chunk pipeline |

## Code Targets

### h-main: Add cudaMemcpyAsync FFI + implement pipelined bounce

**File 1:** `components/gpu-services/v0/src/cuda_ffi.rs`
- After line 111 (end of extern block): Add `cudaStream_t` as `*mut c_void`, plus extern fns: `cudaStreamCreate`, `cudaStreamSynchronize`, `cudaStreamDestroy`, `cudaMemcpyAsync` (same signature as cudaMemcpy + stream param).
- **Why here:** All CUDA FFI is centralized in this file. The bindings are hand-written (not bindgen) for auditability.

**File 2:** `components/gpu-services/v0/src/bin/p2p_server.rs`
- Line 28-36 (`TransferMode` enum): Add `BouncePipelined` variant with doc comment.
- Line 374-433 (`handle_bounce`): Add `Instant` timing around `do_chunked_read` and copy loop. Append `read_us=N copy_us=N total_us=N` to response.
- Line 436-490 (`handle_p2p`): Same timing instrumentation.
- After `handle_p2p_cold` (after line 567): Add new `handle_bounce_pipelined` function implementing double-buffer pipeline with individual ReadAsync + cudaMemcpyAsync.
- Line 569-678 (`main()`): Add routing for new mode variant.

**Pipeline algorithm (for executor):**
```
create 2 host DMA buffers (A, B)
create 1 CUDA stream
connect_client() to get NVMe channels

issue ReadAsync chunk[0] into buf_A
for i in 1..num_chunks:
    wait for completion of chunk[i-1]
    launch cudaMemcpyAsync from completed buf into client_dev_ptr+offset, on stream
    swap active buffer
    issue ReadAsync chunk[i] into swapped buf
wait for last completion
launch last cudaMemcpyAsync
cudaStreamSynchronize(stream)
```

### h-robustness: 2-stream variant

Same as h-main but create 2 streams and round-robin: chunks 0,2,4... on stream_a; chunks 1,3,5... on stream_b. Both streams synchronized at end.

## What I Tried That Didn't Work

- **Looked for cudaMemcpyAsync in existing code:** Not present anywhere in the codebase.
- **Looked for CUDA stream types:** Not in FFI bindings.
- **Checked if SPDK DMA buffers are CUDA-registered:** They are NOT explicitly registered with `cudaHostRegister`. SPDK hugepages may or may not satisfy CUDA's pinned-memory requirement for async copies. If they don't, `cudaMemcpyAsync` will silently fall back to synchronous. **The executor must verify this** — either pre-register with `cudaHostRegister` or check behavior empirically.

## What I Excluded and Why

- **Chunk size variation (64K, 256K, 512K):** Campaign fixes chunk size at 128 KiB. Would change per-chunk timing balance but doesn't test the pipelining mechanism itself.
- **P2P cold mode:** Already characterized (~7.4ms). Not relevant to pipelining comparison.
- **cudaEventRecord for GPU-side timing:** Would add precision but requires more FFI additions. `Instant::now()` with `cudaStreamSynchronize` captures end-to-end correctly.
- **Multi-threaded pipelining (separate NVMe thread + copy thread):** Adds complexity. Single-threaded with async CUDA should work since NVMe completions arrive via channel recv (blocking is fine — it just means the pipeline stalls naturally when NVMe is the bottleneck).
- **Larger transfer sizes (16MB, 64MB):** Would amortize pipeline startup but changes the experimental variable. Keep 4 MiB per campaign spec.

## Evolution of Thinking

Prior iterations treated the hypothesis backwards — testing existing (non-pipelined) bounce against P2P, finding it slower, and declaring the hypothesis "refuted." But the hypothesis explicitly says "pipelined transfers" — which don't exist in the code.

This iteration corrects that by implementing the actual mechanism under test. The iter-2 phase decomposition data (read≈790μs, copy≈819μs, independent hardware, no interference) provides the strongest possible prior for predicting pipelining success: two nearly-equal-duration phases on independent hardware paths = textbook pipeline candidate.

The key uncertainty is whether SPDK hugepage buffers satisfy CUDA's pinned-memory requirement for truly asynchronous `cudaMemcpyAsync`. If they don't, the copy will block and no overlap occurs. The executor should verify this early with a simple test.

## Current Status

- **Validated:** Build command works. CLI flags confirmed. Phase decomposition data from iter-2 provides quantitative predictions. NVMe ReadAsync interface confirmed for per-chunk completions.
- **Uncertain:** Whether SPDK DMA buffers work with `cudaMemcpyAsync` without explicit `cudaHostRegister`. Whether individual ReadAsync has different per-op latency than BatchSubmit. Whether CUDA stream launch overhead is negligible at 32 operations.
- **Suggested next:** If pipelining works (latency ~820μs), iteration 2 should test whether it degrades at smaller transfers (where pipeline startup cost dominates) or larger transfers (where it should be even more effective). If pipelining fails due to SPDK buffer incompatibility with async CUDA, iteration 2 should test with explicit `cudaHostRegister` on the DMA buffers.

## Warnings & Constraints

- **SPDK DMA buffers might not be CUDA-pinned.** `cudaMemcpyAsync` from non-pinned memory silently falls back to synchronous. The executor MUST verify by either: (a) calling `cudaHostRegister` on the DMA buffer pointer before async copies, or (b) timing a known-async vs known-sync path. If async doesn't work, try `cudaHostRegister(buf.as_ptr(), size, 0)` on each DMA buffer after allocation.
- **Individual ReadAsync sends one completion per op.** The channel `completion_rx.recv()` will block until that specific read completes. This is the pipeline's "wait" step — it's fine because during this wait, a prior cudaMemcpyAsync is in flight on the GPU.
- **CUDA stream creation is cheap but not free (~microseconds).** Create the stream once in the handler (before the loop), not per-chunk.
- **Server is single-threaded.** All pipelining must happen within a single thread using async CUDA operations. The NVMe read "blocks" on recv but the GPU copy engine runs independently.
- **MDTS 128 KiB.** Do not increase chunk_size.
- **NVMe PCI: 0000:62:00.0.** Only one SPDK process at a time.
- **Build: `RUSTFLAGS='-L /usr/local/lib'`** for libgdrapi.
- **5s server startup.** The `run_condition.sh` sleeps 5s; don't reduce.
- **Response parsing:** Client checks `resp.startswith("OK")`. Appending timing fields is safe. Parse with regex in client if needed.
