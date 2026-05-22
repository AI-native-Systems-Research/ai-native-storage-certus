# Problem Framing — h8-v1-true-pipeline Iteration 2

## Research Question

Does dispatcher v1 with true overlapped pipelining using `cudaHostAlloc`-allocated staging buffers (natively CUDA-pinned) achieve significantly faster SSD-tier lookup latency than (a) the iter-1 async pipeline that uses `cudaHostRegistered` mmap'd memory-tier as the cudaMemcpyAsync source, and (b) direct P2P SSD→GPU DMA, for 4 MiB lookups (32×128 KiB chunks) through certus-server?

**Mechanism under study:** `cudaMemcpyAsync` from `cudaHostAlloc`-allocated memory enables true asynchronous DMA engine transfers (proven in h8-pipelined campaign), whereas `cudaMemcpyAsync` from `cudaHostRegistered` mmap'd memory falls back to synchronous execution (confirmed in iter-1: only 10% improvement). By staging data through natively pinned buffers, the GPU DMA copy of chunk N can fully overlap with the NVMe read of chunk N+1.

**Key source files:**
- `components/dispatcher/v1/src/pipeline.rs:30-123` — `pipelined_ssd_to_gpu` function (rewrite target)
- `components/gpu-services/v0/src/cuda_ffi.rs:68-111` — CUDA FFI declarations (add stream/async APIs)
- `components/gpu-services/v0/src/dma.rs:253-288` — `create_spdk_dma_buffer_from_cuda_host_alloc` (creates dual CUDA+SPDK DmaBuffers)
- `apps/certus-server/src/main.rs:190-209` — cudaHostRegister on memory-tier pool (existing, insufficient for true async)

## System Interface

- **Build:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server`
  - Code evidence: workspace Cargo.toml:66 defines `dispatcher-v1`, apps/certus-server/Cargo.toml:13 depends on it
  - Feature unification: certus-server enables `gpu` on gpu-services (Cargo.toml:19), dispatcher-v1 enables `spdk` (dispatcher-v1/Cargo.toml:14). Combined: both `gpu` and `spdk` → `dma` module and `create_spdk_dma_buffer_from_cuda_host_alloc` available.

- **Run server:**
  ```bash
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  sudo target/debug/certus-server \
    --metadata-pci 0000:62:00.0 \
    --data-pci 0000:63:00.0 \
    --dispatcher-version v1 \
    --listen 0.0.0.0:50051
  ```
  - Code evidence: apps/certus-server/src/main.rs:174-258 (v1 dispatcher path), clap args defined in main.rs

- **Run benchmark:**
  ```bash
  python3 apps/certus-server/python-client/test_client.py \
    --server localhost:50051 \
    --bench-only \
    --bench-object-size 4194304 \
    --bench-num-objects 10 \
    --bench-iterations 1
  ```
  - Code evidence: test_client.py:444-466 (argparse definitions), test_client.py:310 (`bench_lookup_latency` function)

- **Output format:** Stdout table with Memory-tier and SSD-tier rows. Key metric: SSD-tier `Avg (us/obj)`.

- **CLI flags:**
  - `--dispatcher-version v1`: selects memory-tier caching dispatcher (main.rs:174)
  - `--metadata-pci`: SPDK NVMe device for metadata/extent-manager
  - `--data-pci`: SPDK NVMe device for data I/O (63:00.0 = NODE-level to GPU)
  - `--bench-only`: skips functional tests (test_client.py:475)
  - `--bench-iterations 1`: single lookup per SSD object for cold-hit measurement

## Baseline Command

```bash
# Server (terminal 1):
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
sudo target/debug/certus-server \
  --metadata-pci 0000:62:00.0 \
  --data-pci 0000:63:00.0 \
  --dispatcher-version v1 \
  --listen 0.0.0.0:50051

# Client (terminal 2):
python3 apps/certus-server/python-client/test_client.py \
  --server localhost:50051 \
  --bench-only \
  --bench-object-size 4194304 \
  --bench-num-objects 10 \
  --bench-iterations 1
```

## Baseline Validation

From iter-1 (same hardware, same branch):
- Sequential v1 baseline: **21,361 us/obj** SSD-tier, 0.20 GB/s (results/h-main/baseline-seed42.json)
- Memory-tier: 568.7 us/obj
- Build exits 0 in 0.17s (validated this session)

## Experimental Conditions

### Condition A: Baseline (sequential v1, unpatched)

Run unmodified certus-server with dispatcher v1. This produces the sequential per-chunk pipeline: ReadSync → memcpy → sync cudaMemcpy per chunk.

### Condition B: h-main (cudaHostAlloc staging pipeline)

Modify `pipeline.rs` to implement true double-buffered async with `cudaHostAlloc` staging:
1. At function entry: allocate 2 × chunk_size staging buffers via `cudaHostAlloc`, register with SPDK via `create_spdk_dma_buffer_from_cuda_host_alloc`, create a CUDA stream.
2. Issue ReadAsync for chunk 0 directly into staging buffer 0 (which IS an SPDK DmaBuffer since it's SPDK-registered).
3. Loop: recv ReadDone for current staging buffer → CPU memcpy to memory-tier slot → `cudaMemcpyAsync` from staging buffer to GPU (on CUDA stream) → issue ReadAsync for next chunk into the other staging buffer → swap.
4. After final chunk: `cudaStreamSynchronize`, destroy stream, free staging buffers.

Key difference from iter-1: NVMe reads directly into `cudaHostAlloc` staging buffers (no intermediate DMA buffer → memcpy → mem-tier → cudaMemcpyAsync chain). The staging buffers serve triple duty: NVMe DMA target, source for GPU async copy, and source for mem-tier copy.

### Condition C: h-control-negative (P2P direct DMA)

Same P2P implementation as iter-1 (patches/h-control-negative.patch): prepare_memory_for_spdk + ReadSync into GPU BAR1. This eliminates all staging buffers entirely.

## Success Criteria

1. **h-main:** cudaHostAlloc staging pipeline must reduce SSD-tier latency by more than the iter-1 result (>10% over sequential baseline). The predicted improvement is 30-50% (vs iter-1's 10%) because true async GPU DMA fully hides the H2D copy latency behind NVMe reads.
2. **h-control-negative:** P2P must maintain its relative advantage over baseline (>20% faster, matching iter-1's 23%), confirming mechanism consistency across runs.
3. **Relative ordering:** If `cudaHostAlloc` enables true async: pipeline should approach P2P performance (both hide copy latency; pipeline has extra mem-tier memcpy overhead). Expected: P2P ≤ cudaHostAlloc-pipeline < sequential.

## Constraints

- All benchmarks through certus-server (no standalone binaries)
- Do NOT modify gpu-p2p-server
- Implementation changes only in components/dispatcher/v1/
- Use --bench-iterations 1 for first-hit comparison
- Restart server between conditions (fresh process, fresh populate cycle)
- sudo required for SPDK
- RUSTFLAGS='-L /usr/local/lib' for build, LD_LIBRARY_PATH at runtime

## Prior Knowledge

- **RP-13 (medium):** cudaMemcpyAsync from cudaHostRegistered mmap'd memory provides only partial async benefit (~10%). CONFIRMED in iter-1.
- **RP-14 (medium):** P2P direct SSD→GPU DMA is consistently faster than memory-tier pipeline. CONFIRMED directionally in iter-1 (23% faster), though absolute magnitude 3-5x higher than historical.
- **RP-15 (high):** Ordering P2P < async_pipeline < sequential preserved under load. CONFIRMED in iter-1.
- **h8-pipelined finding:** `cudaHostAlloc` + SPDK registration enables true async GPU DMA, proven to give 2.4-3x speedup in bounce-buffer experiments. This is the basis for iter-2's approach.
