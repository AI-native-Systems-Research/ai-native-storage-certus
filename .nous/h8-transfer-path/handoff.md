# Handoff: Phase Decomposition of Bounce vs P2P (Iteration 2)

## Goal

Decompose the established 2x latency gap between bounce and P2P warm into its constituent phases (NVMe read vs memory copy) to determine whether the copy type (H2D vs D2D) is the sole mechanism, or whether NVMe DMA target selection also contributes.

## Key Discoveries

- **Both modes use BatchSubmit for concurrent NVMe reads** — all 32 chunks are submitted as a single batch (`do_chunked_read` at `p2p_server.rs:272`). The NVMe I/O is NOT pipelined with the copy phase; reads complete fully before copies begin. This is a "read-all-then-copy-all" design, not a streaming pipeline.
- **Bounce host buffers are SPDK hugepage allocations** — `DmaBuffer::new(chunk_size, sector_size, None)` at line 388 allocates from SPDK's DMA-safe hugepage pool. These have pre-configured IOMMU mappings.
- **P2P staging uses GDRCopy BAR1 mapping** — `create_spdk_dma_buffer_from_gpu_bar` (dma.rs:353) pins GPU memory via `gdr_pin_buffer`, maps it to CPU VA via `gdr_map`, then registers with SPDK via `spdk_mem_register`. The BAR1 VA is what NVMe DMA targets.
- **Both `cudaMemcpy` calls are synchronous** — `CUDA_MEMCPY_HOST_TO_DEVICE` (line 415) and `CUDA_MEMCPY_DEVICE_TO_DEVICE` (line 471) use the default (NULL) CUDA stream, blocking until completion. Timing them with `Instant::now()` captures true GPU completion time.
- **No timing instrumentation exists in the server** — entirely client-side wall-clock measurement currently. `std::time::Instant` is not imported.
- **Response format is flexible** — client checks `resp.startswith("OK")` only (line 121, 129). Appending `read_us=N copy_us=N` is safe.
- **No `cudaMemcpyAsync` exists in the codebase** — all copies are blocking `cudaMemcpy`. No CUDA stream infrastructure present.
- **P2P-cold allocates 32 fresh GDRCopy handles per request** — ~6ms overhead (iter-1 confirmed).
- **`cudaDeviceSynchronize` is available** in FFI at `cuda_ffi.rs:76` but not needed since cudaMemcpy on default stream is synchronous.
- **RUSTFLAGS required for build** — `RUSTFLAGS='-L /usr/local/lib'` is mandatory because `libgdrapi.so` is at `/usr/local/lib`.

## System Interface

- **Build:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server`
- **Run baseline (instrumented bounce):**
  ```bash
  bash .nous/h8-transfer-path/runs/iter-2/inputs/run_condition.sh \
    bounce results/instrumented-bounce-s1.txt 0000:62:00.0
  ```
- **Output format:** Client prints benchmark table to stderr with Throughput (MB/s), Avg/Min/Max latency (ms). After instrumentation, will also report per-phase breakdown (read_us, copy_us averages). The `run_condition.sh` harness captures combined output to the specified file.
- **Baseline result (validated on hardware, iter-1):**
  - Bounce: 1544.0 MB/s throughput, 2.59 ms avg latency
  - P2P warm: 3064.0 MB/s throughput, 1.31 ms avg latency
  - P2P cold: 541.4 MB/s throughput, 7.39 ms avg latency
- **Runtime environment:** `LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64` (set by `run_condition.sh`)

## Code Map

| Location | What | When to look |
|----------|------|-------------|
| `components/gpu-services/v0/src/bin/p2p_server.rs:38-64` | CLI argument definitions (Cli struct) | Adding --skip-nvme flag |
| `components/gpu-services/v0/src/bin/p2p_server.rs:272-322` | `do_chunked_read` — BatchSubmit of NVMe reads | Understanding NVMe read timing boundary |
| `components/gpu-services/v0/src/bin/p2p_server.rs:374-433` | `handle_bounce` — full bounce path | Adding timing around lines 399 (read) and 406-427 (copy loop) |
| `components/gpu-services/v0/src/bin/p2p_server.rs:436-490` | `handle_p2p` — warm P2P path | Adding timing around lines 461 (read) and 467-485 (copy loop) |
| `components/gpu-services/v0/src/bin/p2p_server.rs:493-567` | `handle_p2p_cold` — cold P2P path | Not instrumented this iteration |
| `components/gpu-services/v0/src/bin/p2p_server.rs:569-678` | `main()` — CLI parsing and request loop | Threading --skip-nvme to handlers |
| `components/gpu-services/v0/src/cuda_ffi.rs:76` | `cudaDeviceSynchronize` FFI declaration | Not needed — cudaMemcpy is synchronous |
| `components/gpu-services/v0/src/dma.rs:353-466` | `create_spdk_dma_buffer_from_gpu_bar` — GDRCopy BAR1 setup | If P2P DMA fails |
| `components/gpu-services/v0/tests/gpu_client_p2p.py:44-61` | `do_transfer` — client timing + response parsing | Adding phase timing extraction |
| `components/gpu-services/v0/tests/gpu_client_p2p.py:116-151` | Benchmark report formatting | Adding phase breakdown table |
| `components/block-device-spdk-nvme/v1/src/actor.rs:636-652` | BatchSubmit dispatch loop | If NVMe completions are missing |

## Code Targets

### h-main: Per-phase instrumentation

**File:** `components/gpu-services/v0/src/bin/p2p_server.rs`

**handle_bounce (lines 374-433):**
- Before line 399 (`do_chunked_read`): `let read_start = std::time::Instant::now();`
- After line 403 (after the if-let returns Ok): `let read_us = read_start.elapsed().as_micros();`
- Before line 406 (copy loop): `let copy_start = std::time::Instant::now();`
- After line 427 (end of copy loop): `let copy_us = copy_start.elapsed().as_micros();`
- Line 432 (Ok response): append `read_us={read_us} copy_us={copy_us}`

**handle_p2p (lines 436-490):**
- Before line 461 (`do_chunked_read`): `let read_start = std::time::Instant::now();`
- After line 464 (after the if-let): `let read_us = read_start.elapsed().as_micros();`
- Before line 467 (D2D copy loop): `let copy_start = std::time::Instant::now();`
- After line 485 (end of copy loop): `let copy_us = copy_start.elapsed().as_micros();`
- Line 489 (Ok response): append `read_us={read_us} copy_us={copy_us}`

**Client:** `components/gpu-services/v0/tests/gpu_client_p2p.py`
- Parse `read_us=(\d+)` and `copy_us=(\d+)` from each response string.
- Report avg/min/max for each phase in the benchmark summary.

### h-ablation: --skip-nvme flag

**File:** `components/gpu-services/v0/src/bin/p2p_server.rs`
- Add `#[arg(long)] skip_nvme: bool` to Cli struct (around line 62)
- Pass `cli.skip_nvme` through to handler functions (adjust signatures)
- In handle_bounce: wrap `do_chunked_read` in `if !skip_nvme { ... }`
- In handle_p2p: same treatment
- When skip_nvme is true, set `read_us = 0`

## What I Tried That Didn't Work

- Looked for `cudaMemcpyAsync` or CUDA stream usage — not present. Cannot overlap copies with reads without adding stream infrastructure.
- Looked for any existing per-phase timing or profiling — none exists.
- Checked for `cudaEventRecord`/`cudaEventElapsedTime` — not in the FFI bindings. `Instant::now()` is sufficient because cudaMemcpy is synchronous.
- The `apps/gpu-handle-test-client/client.py` uses incompatible protocol (length-prefixed binary vs line-based base64). Do NOT use it.

## What I Excluded and Why

- **cudaMemcpyAsync with CUDA streams (pipelining):** Requires significant code infrastructure (stream creation, async coordination). Deferred to iteration 3 if copy-dominance mechanism is confirmed.
- **Varying chunk sizes (64K, 256K, 512K):** Campaign specifies 128 KiB. Chunk size variation could reveal if H2D overhead is per-chunk or per-byte. Deferred to iteration 3.
- **PCIe topology investigation (lspci -tv):** Informative but doesn't change experiment design. Noted in h-control-negative diagnostic.
- **cudaEventRecord for GPU-side timing:** More precise but requires new FFI bindings. Instant is sufficient.
- **P2P cold mode instrumentation:** Already characterized in iter-1. Not relevant to phase decomposition.
- **Pipelining (read-copy overlap):** Would confound the phase decomposition measurement. Test isolation first, optimize later.

## Evolution of Thinking

Iteration 1 established the 2x gap and proposed the diagnostic: "the extra cudaMemcpy H2D step in bounce mode is the bottleneck." But this was inference from total latency, not direct measurement. Iteration 2 makes it directly observable with per-phase timing.

Added the ablation arm (--skip-nvme) to detect a subtle confound: NVMe DMA traffic might cause PCIe bus congestion that disproportionately slows subsequent H2D copies (same PCIe bus) but not D2D copies (GPU-internal). If copy-only and full-path copy times match, this confound doesn't exist. If they differ, it reveals an NVMe-copy interaction effect that would be important for any pipelining optimization in iteration 3.

## Current Status

- **Validated:** Code paths for instrumentation identified with exact line numbers. Response format extensibility confirmed. cudaMemcpy synchronous semantics confirmed. Build command validated in iter-1. Timing with Instant is appropriate (nanosecond resolution on Linux CLOCK_MONOTONIC).
- **Uncertain:** Whether NVMe read to BAR1 takes the same time as to host memory (h-control-negative tests this). Whether PCIe congestion from NVMe reads affects H2D differently than D2D (ablation tests this).
- **Suggested next:** If h-main confirms copy phase dominates, iteration 3 should test pipelining (overlap NVMe reads with copies using CUDA streams) or test whether larger chunk sizes (256K+) change the H2D/D2D ratio.

## Warnings & Constraints

- **SPDK exclusive device access**: The NVMe device must be bound to `vfio-pci`. Only one SPDK process can use it at a time. Kill any existing server before starting a new one.
- **Server startup time**: SPDK+CUDA initialization takes ~5 seconds. The `run_condition.sh` uses `sleep 5` before client connection.
- **Socket cleanup**: The server removes its socket on clean exit. If it crashes, `rm /tmp/gpu_p2p_bench_*.sock` before restarting.
- **GDRCopy 64K alignment**: GPU staging buffers aligned to 64 KiB GPU page size. 128 KiB chunks are fine.
- **MDTS constraint**: 128 KiB is the NVMe MDTS limit (`p2p_server.rs:57`). Do not increase chunk_size.
- **Build requires RUSTFLAGS**: `RUSTFLAGS='-L /usr/local/lib'` for `libgdrapi.so`.
- **NVMe PCI address**: Test machine's NVMe at `0000:62:00.0`.
- **cudaMemcpy is synchronous on default stream**: No cudaDeviceSynchronize needed after timing.
- **--skip-nvme produces garbage data**: Buffers contain zeros/stale data. Fine for timing — cudaMemcpy copies at full speed regardless of content.
- **Timing resolution**: `Instant::now()` on Linux uses CLOCK_MONOTONIC with nanosecond precision. Microsecond reporting appropriate for 100us-2000us range.
- **The --staging-size flag only affects P2P mode** (pre-allocates chunk pool). Harmless to include for bounce.
- **Client measures wall-clock** from socket send to response received — includes socket RTT (~tens of microseconds, negligible for ms-scale).
