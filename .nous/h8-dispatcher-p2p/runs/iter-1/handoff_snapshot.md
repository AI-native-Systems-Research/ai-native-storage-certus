# Handoff — h8-dispatcher-p2p, Iteration 1

## Goal

Measure end-to-end latency and throughput of three NVMe→GPU transfer paths (bounce, P2P warm, P2P cold) for 4 MiB payloads at 128 KiB MDTS chunk size, and determine whether the existing bounce-buffer pipeline in the dispatcher is faster or slower than direct P2P via GDRCopy BAR1 mapping.

## Key Discoveries

1. **P2P warm is ~2.9x faster than bounce** in initial probes: 3002 MB/s / 1.33ms vs 1045 MB/s / 3.83ms for 4 MiB on this hardware (A30 GPU, Intel P5800X-class NVMe).
2. **The p2p_server binary already supports all three transfer modes** — no code changes needed for the main comparison. Build with `RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server`.
3. **GDRCopy pin+map+SPDK register takes ~6ms per call** (from prior experiment notes). This means P2P cold will add 6ms × 32 chunks = ~192ms overhead on top of the pure DMA time, making it slower than bounce for 4 MiB.
4. **VFIO group 14 (device 0000:62:00.0) is busy** after being opened once in a session. Use `0000:63:00.0` instead. Clear `/var/tmp/spdk_pci_lock_*` before each run.
5. **Batch NVMe reads via BatchSubmit** (p2p_server.rs:286) issue all chunks concurrently to the controller queue, achieving much better parallelism than the dispatcher's per-chunk ReadSync loop (pipeline.rs:60-119).
6. **Client measures end-to-end including Unix socket round-trip** — the socket overhead is negligible (<0.1ms) but consistent across modes, so it cancels out.
7. **Both A30 GPUs are on NUMA node 0**, same as NVMe devices 61-64. No cross-NUMA effects.

## System Interface

- **Build:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server
  ```
- **Run baseline (bounce):**
  ```bash
  rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 /tmp/gpu_p2p_bench.sock && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  ./target/debug/gpu-p2p-server --socket /tmp/gpu_p2p_bench.sock --pci 0000:63:00.0 --mode bounce --chunk-size 131072 --staging-size 4194304
  ```
- **Run client:**
  ```bash
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  python3 components/gpu-services/v0/tests/gpu_client_p2p.py 4194304 /tmp/gpu_p2p_bench.sock --iterations 20
  ```
- **Output format:** Client prints to stderr: `Throughput: X MB/s`, `Avg latency: X ms`, `Min latency: X ms`, `Max latency: X ms`. Parse these.
- **Baseline result:** Bounce 1044.9 MB/s avg, P2P warm 3002.3 MB/s avg (10 iterations each).

## Code Map

| File:Line | What's there | When to look |
|-----------|-------------|--------------|
| `components/gpu-services/v0/src/bin/p2p_server.rs:28-36` | TransferMode enum (bounce/p2p/p2p-cold) | Understanding mode selection |
| `components/gpu-services/v0/src/bin/p2p_server.rs:374` | `handle_bounce`: NVMe→host→cudaMemcpy H2D | If bounce results are unexpected |
| `components/gpu-services/v0/src/bin/p2p_server.rs:436` | `handle_p2p`: NVMe→GPU staging→D2D copy | If P2P warm results are unexpected |
| `components/gpu-services/v0/src/bin/p2p_server.rs:493` | `handle_p2p_cold`: per-request pin/unpin | If cold P2P setup cost is different than expected |
| `components/gpu-services/v0/src/bin/p2p_server.rs:271-323` | `do_chunked_read`: BatchSubmit of async reads | If NVMe read phase is slow |
| `components/gpu-services/v0/src/dma.rs:353` | `create_spdk_dma_buffer_from_gpu_bar` | If GDRCopy/SPDK registration fails |
| `components/gpu-services/v0/tests/gpu_client_p2p.py:44-61` | `do_transfer`: socket send/recv timing | If latency numbers include unexpected overhead |
| `components/dispatcher/v1/src/pipeline.rs:30-123` | `pipelined_ssd_to_gpu`: sequential per-chunk ReadSync | Context for dispatcher integration |

## Code Targets

No code changes required for the main experiment arms. All three conditions use the existing `gpu-p2p-server` binary with different `--mode` flags.

For the robustness arm (64 KiB chunks): use `--chunk-size 65536` flag (no code change).

## What I Tried That Didn't Work

- **Device 0000:62:00.0:** VFIO group 14 becomes busy ("Cannot open /dev/vfio/14: Device or resource busy") after the first SPDK session opens it. The kernel doesn't release the VFIO group FD immediately. Using 0000:63:00.0 avoids this.
- **Running server with `--once`:** When using `--iterations N` in the client, the server exits after the warmup transfer, causing the benchmark loop to fail with "No such file or directory". Always run without `--once` for benchmark iterations.
- **Stale SPDK locks:** `/var/tmp/spdk_pci_lock_*` files from prior sessions block initialization. Always `rm -f` the target device's lock before starting the server.

## What I Excluded and Why

- **Dispatcher-level integration (Condition D in problem.md):** Excluded from the bundle because adding P2P to `pipeline.rs` requires significant code changes (new function, GDRCopy setup, SPDK registration lifecycle) that go beyond what this iteration should validate. The standalone p2p_server already isolates the DMA path comparison. Integration is a follow-up for iteration 2.
- **Cross-NUMA tests:** Both GPUs and all NVMe drives used are on NUMA 0. Cross-NUMA would add interconnect latency but doesn't test the P2P mechanism itself.
- **Larger transfer sizes (16 MiB, 64 MiB):** The research question specifies 4 MiB. Larger sizes are an obvious follow-up but would dilute the core comparison.
- **Debug vs release build:** Using debug build for this iteration because the dominant cost is DMA hardware operations (PCIe transfers), not CPU computation. The ~1ms latencies are DMA-bound, not instruction-bound. Release build would mainly speed up the BatchSubmit command preparation, which is negligible.

## Evolution of Thinking

1. **Initial assumption:** The research question implies bounce might be faster — perhaps due to pipelining overlap or DRAM tier caching benefits.
2. **After reading pipeline.rs:** The "pipelined" implementation is actually sequential (ReadSync → memcpy → DMA per chunk). No true overlap. This removes the main theoretical advantage of bounce.
3. **After running probes:** P2P warm is decisively faster (2.9x). The hypothesis appears false. The experiment will confirm this with statistical rigor.
4. **Key insight:** The p2p_server's bounce mode uses BatchSubmit (concurrent reads), which is *better* than the dispatcher's sequential pipeline. Yet P2P warm still dominates — meaning the path savings (eliminating host DRAM hop) outweigh any submission strategy differences.

## Current Status

- **Validated:** Build command works, both server modes produce correct results, client reports clean metrics, devices 0000:63:00.0 and GPUs available.
- **Uncertain:** Whether P2P cold will be slower or faster than bounce (6ms × 32 chunks would make it ~192ms total — much worse — but GDRCopy may batch internally). Need actual measurement.
- **Suggested next:** If P2P warm confirms dominant advantage, iteration 2 should implement P2P path in dispatcher's `promote_and_serve` (components/dispatcher/v1/src/lib.rs:190) and measure end-to-end dispatcher lookup latency with real KV-cache entries.

## Warnings & Constraints

- **SPDK singleton:** Only one SPDK environment can be active per process (and per VFIO group). Starting the server twice on the same device will fail. Kill the previous server process before starting a new one.
- **Socket cleanup:** Always `rm -f` the socket path before starting the server. A stale socket causes bind failures.
- **Server startup time:** Takes 3-5 seconds (SPDK init + CUDA init + GDRCopy pool allocation for P2P mode). Client must wait before connecting.
- **P2P mode requires 32 GPU staging buffers:** Each is 128 KiB of pinned GPU VRAM. For 4 MiB with 128 KiB chunks, this uses 4 MiB of extra GPU memory.
- **VFIO group recovery:** After a server crashes or is killed, the VFIO group may remain locked for ~10 seconds. If a device fails, try a different one from the same NUMA node.
- **Client output is to stderr:** Parse benchmark results from stderr, not stdout.
