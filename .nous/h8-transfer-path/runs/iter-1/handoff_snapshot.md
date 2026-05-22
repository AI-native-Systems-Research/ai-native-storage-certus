# Handoff: Bounce vs P2P Transfer Path (Iteration 1)

## Goal

Measure and compare end-to-end latency and throughput of three NVMe→GPU transfer modes (bounce, p2p-warm, p2p-cold) for 4 MiB transfers broken into 32 x 128 KiB chunks. Determine whether the two-stage bounce buffer path is faster than direct GPU DMA.

## Key Discoveries

- **Both modes use BatchSubmit for concurrent NVMe reads** — all 32 chunks are submitted as a single batch (`do_chunked_read` at `p2p_server.rs:272`). The NVMe I/O is NOT pipelined with the copy phase; reads complete fully before copies begin. This is a "read-all-then-copy-all" design, not a streaming pipeline.
- **Bounce host buffers are SPDK hugepage allocations** — `DmaBuffer::new(chunk_size, sector_size, None)` at line 388 allocates from SPDK's DMA-safe hugepage pool. These have pre-configured IOMMU mappings.
- **P2P staging uses GDRCopy BAR1 mapping** — `create_spdk_dma_buffer_from_gpu_bar` (dma.rs:353) pins GPU memory via `gdr_pin_buffer`, maps it to CPU VA via `gdr_map`, then registers with SPDK via `spdk_mem_register`. The BAR1 VA is what NVMe DMA targets.
- **The copy phase differs**: bounce does `cudaMemcpy(H2D)` from host→device per chunk; p2p does `cudaMemcpy(D2D)` from staging GPU→client GPU per chunk. Both are synchronous (no CUDA streams used).
- **No `cudaMemcpyAsync` exists in the codebase** — all copies are blocking `cudaMemcpy`. No CUDA stream infrastructure is present.
- **P2P-cold allocates 32 fresh GDRCopy handles per request** — each involving `cudaMalloc` + `gdr_open` + `gdr_pin_buffer` + `gdr_map` + `spdk_mem_register`, then full teardown on completion.
- **Client measures wall-clock latency** from socket send to response received (`time.perf_counter()` at `gpu_client_p2p.py:49,58`). This includes Unix socket RTT overhead (~tens of microseconds, negligible for ms-scale transfers).
- **RUSTFLAGS required for build** — `RUSTFLAGS='-L /usr/local/lib'` is mandatory because `libgdrapi.so` is at `/usr/local/lib`, not in the default linker search path.

## System Interface

- **Build:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server`
- **Run baseline (bounce):**
  ```bash
  bash .nous/h8-transfer-path/runs/iter-1/inputs/run_condition.sh \
    bounce results/h-main/bounce-s1.txt 0000:62:00.0
  ```
- **Output format:** Client prints benchmark table to stderr with Throughput (MB/s), Avg/Min/Max latency (ms). The `run_condition.sh` harness captures combined output to the specified file.
- **Baseline result (validated on hardware):**
  - Bounce: 1544.0 MB/s throughput, 2.59 ms avg latency
  - P2P warm: 3064.0 MB/s throughput, 1.31 ms avg latency
  - P2P cold: 541.4 MB/s throughput, 7.39 ms avg latency
- **Runtime environment:** `LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64` (set by `run_condition.sh`)

## Code Map

| Location | What | When to look |
|----------|------|-------------|
| `components/gpu-services/v0/src/bin/p2p_server.rs:38-64` | CLI argument definitions (Cli struct) | If flags seem wrong or need new ones |
| `components/gpu-services/v0/src/bin/p2p_server.rs:272-322` | `do_chunked_read` — BatchSubmit of NVMe reads | If NVMe read latency is suspect |
| `components/gpu-services/v0/src/bin/p2p_server.rs:374-433` | `handle_bounce` — full bounce path | Main comparison point |
| `components/gpu-services/v0/src/bin/p2p_server.rs:436-490` | `handle_p2p` — warm P2P path | Main comparison point |
| `components/gpu-services/v0/src/bin/p2p_server.rs:493-567` | `handle_p2p_cold` — cold P2P path | Control negative |
| `components/gpu-services/v0/src/dma.rs:353-466` | `create_spdk_dma_buffer_from_gpu_bar` — GDRCopy BAR1 setup | If P2P DMA fails |
| `components/gpu-services/v0/tests/gpu_client_p2p.py:46-60` | `do_transfer` — client timing logic | If latency numbers seem off |
| `components/gpu-services/v0/tests/gpu_client_p2p.py:116-151` | Benchmark report formatting | To parse output |
| `components/block-device-spdk-nvme/v1/src/actor.rs:636-652` | BatchSubmit dispatch loop | If NVMe completions are missing |

## Code Targets

No code changes needed for this iteration. All three conditions are exercised via the existing `--mode` flag.

## What I Tried That Didn't Work

- Looked for `cudaMemcpyAsync` or CUDA stream usage — not present. The system uses only synchronous `cudaMemcpy`.
- Looked for an existing benchmark harness or results file — none exists. The Python client with `--iterations` is the benchmarking mechanism.
- The `apps/gpu-handle-test-client/client.py` uses a different protocol (length-prefixed binary) incompatible with the p2p_server's line-based base64 protocol. Do NOT use it.

## What I Excluded and Why

- **Varying chunk sizes** (64K, 256K, 512K): The campaign specifies 128 KiB. Would be interesting for iteration 2 — smaller chunks amplify per-chunk copy overhead.
- **Pipelining optimization** (overlap NVMe reads with copies): The current code does read-all-then-copy-all. A pipelined version would be a code change experiment for iteration 2.
- **cudaMemcpyAsync with CUDA streams**: Not available in current code. Would require adding stream infrastructure. Relevant for iteration 2.
- **Per-phase timing**: No instrumentation to separately measure NVMe read time vs copy time. Would require code changes. Suggested for iteration 2 to diagnose mechanism if h-main is refuted.

## Evolution of Thinking

Initially expected "pipelining" to be the mechanism (overlapping NVMe reads and H2D copies in bounce mode). Reading the code revealed there is NO pipelining — both modes do read-all-then-copy-all sequentially. The actual mechanism difference is purely about the NVMe DMA target (host memory vs GPU BAR1) and the copy type (H2D vs D2D). This makes the experiment cleaner: it isolates the NVMe DMA target question without confounding pipeline effects.

## Current Status

- **Validated:** CLI flags, client protocol, code paths for all three modes, BatchSubmit concurrency model, GDRCopy setup mechanics, build command (RUSTFLAGS required), run_condition.sh harness, baseline results on hardware.
- **Uncertain:** Whether NVMe read completion time differs between host-DMA and BAR1-DMA targets (requires per-phase instrumentation to decompose). PCIe topology specifics (same switch or different?).
- **Suggested next:** If h-main is refuted (P2P warm wins), iteration 2 should add per-phase instrumentation to decompose latency into NVMe-read phase vs copy phase. This would definitively show whether the NVMe read is equally fast to both targets (implicating the copy type as the differentiator) or whether BAR1 writes are slower (implicating topology/controller effects).

## Warnings & Constraints

- **SPDK exclusive device access**: The NVMe device must be bound to `vfio-pci` (not the kernel nvme driver). Only one SPDK process can use it at a time. Kill any existing server before starting a new one.
- **Server startup time**: SPDK+CUDA initialization takes ~5 seconds. The `run_condition.sh` uses `sleep 5` before client connection.
- **Socket cleanup**: The server removes its socket on clean exit. If it crashes, you must `rm /tmp/gpu_p2p_bench_*.sock` before restarting.
- **GDRCopy 64K alignment**: GPU staging buffers are aligned to 64 KiB GPU page size (`GPU_PAGE_SIZE` at `gdrcopy_ffi.rs:17`). 128 KiB chunks are fine (2x alignment).
- **MDTS constraint**: 128 KiB is stated as the NVMe MDTS limit (`p2p_server.rs:57`). Do not increase chunk_size without verifying MDTS.
- **The `--staging-size` flag only affects P2P mode** (pre-allocates the chunk pool). It's ignored in bounce and p2p-cold modes but harmless to include.
- **Build requires RUSTFLAGS**: `RUSTFLAGS='-L /usr/local/lib'` is needed for linking. Without it, the build fails on missing `libgdrapi`.
- **NVMe PCI address**: The test machine's NVMe is at `0000:62:00.0`. The `run_condition.sh` accepts this as third argument.
