# Handoff: Phase Decomposition of Bounce vs P2P (Iteration 2)

## Goal

Decompose the established 2x latency gap between bounce and P2P warm into its constituent phases (NVMe read vs memory copy) to determine whether the copy type (H2D vs D2D) is the sole mechanism, or whether NVMe DMA target selection also contributes.

## Key Discoveries

- **No timing instrumentation exists in the server** — `std::time::Instant` is not imported, no elapsed time measurement anywhere in p2p_server.rs. All timing is client-side (wall-clock around socket round-trip).
- **`cudaDeviceSynchronize` is available in FFI** — declared at `cuda_ffi.rs:76`. Not currently called anywhere in the hot path. Relevant if cudaMemcpy needs synchronization verification (though cudaMemcpy with default stream is synchronous).
- **Both `cudaMemcpy` calls are synchronous** — `CUDA_MEMCPY_HOST_TO_DEVICE` (line 415) and `CUDA_MEMCPY_DEVICE_TO_DEVICE` (line 471) use the default (NULL) CUDA stream, so they block until completion. No CUDA streams exist in the codebase. Timing them with `Instant::now()` before and after captures true GPU completion time.
- **Response format is flexible** — client checks `resp.startswith("OK")` only (line 121, 129). Any text after "OK" is ignored by the success check. Appending `read_us=N copy_us=N` to the response is safe.
- **The `do_chunked_read` function is shared** between handle_bounce (line 399) and handle_p2p (line 461) — same code path, only the DMA buffer backing differs (host hugepage vs GDRCopy BAR1 GPU memory).
- **Bounce host buffers come from SPDK hugepage pool** — `DmaBuffer::new(chunk_size, sector_size, None)` at line 388. These have IOMMU mappings via SPDK's memory registration.
- **P2P staging uses pre-pinned GDRCopy BAR1 buffers** — pool created at startup (`create_chunk_pool` around line 250), reused across requests. The DMA buffer wraps a GPU BAR1-mapped region.

## System Interface

- **Build:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server`
- **Run baseline (instrumented bounce):**
  ```bash
  bash .nous/h8-transfer-path/runs/iter-2/inputs/run_condition.sh \
    bounce results/instrumented-bounce-s1.txt 0000:62:00.0
  ```
- **Output format:** Client prints benchmark table to stderr with Throughput (MB/s), Avg/Min/Max latency (ms). After code changes, will also report per-phase breakdown (Avg read_us, Avg copy_us). The `run_condition.sh` harness captures combined output to the specified file.
- **Baseline result (from iter-1, validated on hardware):**
  - Bounce: 1544.0 MB/s throughput, 2.59 ms avg latency
  - P2P warm: 3064.0 MB/s throughput, 1.31 ms avg latency

## Code Map

| Location | What | When to look |
|----------|------|-------------|
| `components/gpu-services/v0/src/bin/p2p_server.rs:38-64` | CLI argument definitions (Cli struct) | Adding --skip-nvme flag |
| `components/gpu-services/v0/src/bin/p2p_server.rs:272-322` | `do_chunked_read` — BatchSubmit of NVMe reads | Understanding NVMe read timing boundary |
| `components/gpu-services/v0/src/bin/p2p_server.rs:374-433` | `handle_bounce` — full bounce path | Adding timing around lines 399 (read) and 406-427 (copy loop) |
| `components/gpu-services/v0/src/bin/p2p_server.rs:436-490` | `handle_p2p` — warm P2P path | Adding timing around lines 461 (read) and 467-485 (copy loop) |
| `components/gpu-services/v0/src/bin/p2p_server.rs:569-678` | `main()` — CLI parsing and request loop | Threading --skip-nvme flag to handlers |
| `components/gpu-services/v0/src/cuda_ffi.rs:76` | `cudaDeviceSynchronize` FFI declaration | If sync needed after timing |
| `components/gpu-services/v0/tests/gpu_client_p2p.py:44-61` | `do_transfer` — client timing + response parsing | Adding phase timing extraction |
| `components/gpu-services/v0/tests/gpu_client_p2p.py:116-151` | Benchmark report formatting | Adding phase breakdown table |

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
- In `do_transfer` (line 44): return the response string along with elapsed.
- In benchmark loop: parse `read_us=(\d+)` and `copy_us=(\d+)` from each response.
- In report section: add lines for avg/min/max read_us and copy_us.

### h-ablation: --skip-nvme flag

**File:** `components/gpu-services/v0/src/bin/p2p_server.rs`
- Add to Cli struct (around line 62): `#[arg(long)] skip_nvme: bool`
- In `main()` (around line 642-651): pass `cli.skip_nvme` to `handle_bounce` and `handle_p2p`
- In `handle_bounce`: wrap the `do_chunked_read` call in `if !skip_nvme { ... }`
- In `handle_p2p`: same treatment
- Timing: when skip_nvme is true, set `read_us = 0`

## What I Tried That Didn't Work

- Looked for `cudaMemcpyAsync` or CUDA stream usage — not present. Cannot overlap copies with reads without adding stream infrastructure.
- Looked for any existing per-phase timing or profiling — none exists. Entirely client-side measurement currently.
- Checked for `cudaEventRecord`/`cudaEventElapsedTime` — not in the FFI bindings. Would need adding. `Instant::now()` is sufficient because cudaMemcpy on default stream is synchronous.
- The `apps/gpu-handle-test-client/client.py` uses incompatible protocol — do NOT use it.

## What I Excluded and Why

- **cudaMemcpyAsync with CUDA streams (pipelining reads and copies):** Would require significant code infrastructure (stream creation, async coordination). Saves this for iteration 3 if the mechanism is confirmed — it would be the next optimization to test.
- **Varying chunk sizes (64K, 256K):** Campaign specifies 128 KiB. Chunk size variation could reveal whether H2D overhead is per-chunk (constant) or per-byte (proportional). Deferred to iteration 3.
- **PCIe topology investigation (lspci -tv):** Would inform the mechanism but doesn't change the experiment design. Noted in diagnostic as follow-up if NVMe read times differ.
- **cudaEventRecord for GPU-side timing:** More precise than host-side Instant but requires adding FFI bindings. Instant is sufficient because cudaMemcpy is synchronous — host-side timing captures GPU completion.
- **P2P cold mode:** Already characterized in iter-1 (6ms overhead from GDRCopy). Not relevant to phase decomposition between bounce and P2P warm.

## Evolution of Thinking

Iteration 1 assumed bounce would be faster because NVMe DMA to host memory follows the "native" path. This was wrong — the extra H2D copy in bounce is the bottleneck. But this was diagnosed only from total latency comparison. We don't actually know the split: is it 80% copy / 20% NVMe read, or 50/50 with D2D being much faster at both? The phase decomposition answers this definitively.

The ablation arm (--skip-nvme) was added because there's a subtle confound: NVMe DMA traffic might cause PCIe congestion that affects the subsequent H2D copy differently than it affects D2D. If copy-only times match the full-path copy times, this confound doesn't exist. If they differ, it reveals an NVMe-copy interaction effect.

## Current Status

- **Validated:** Code paths for instrumentation identified with exact line numbers. Response format extensibility confirmed (client only checks "OK" prefix). cudaMemcpy is synchronous so Instant timing is accurate. Build command from iter-1 confirmed working.
- **Uncertain:** Whether NVMe read to BAR1 takes the same time as to host memory (this is what h-control-negative tests). Whether PCIe congestion from NVMe reads affects subsequent H2D differently than D2D (ablation tests this).
- **Suggested next:** If h-main confirms copy phase dominates, iteration 3 should test pipelining (overlap NVMe reads with copies using CUDA streams) to see if bounce can close the gap by hiding copy latency behind read latency. Alternatively, test larger chunk sizes (256K, 512K) to see if the H2D/D2D ratio changes with transfer size.

## Warnings & Constraints

- **SPDK exclusive device access**: The NVMe device must be bound to `vfio-pci`. Only one SPDK process can use it at a time. Kill any existing server before starting a new one.
- **Server startup time**: SPDK+CUDA initialization takes ~5 seconds. The `run_condition.sh` uses `sleep 5` before client connection.
- **Socket cleanup**: The server removes its socket on clean exit. If it crashes, `rm /tmp/gpu_p2p_bench_*.sock` before restarting.
- **GDRCopy 64K alignment**: GPU staging buffers are aligned to 64 KiB GPU page size. 128 KiB chunks are fine.
- **MDTS constraint**: 128 KiB is the NVMe MDTS limit (`p2p_server.rs:57`). Do not increase chunk_size.
- **Build requires RUSTFLAGS**: `RUSTFLAGS='-L /usr/local/lib'` is needed for linking `libgdrapi.so`.
- **NVMe PCI address**: The test machine's NVMe is at `0000:62:00.0`.
- **cudaMemcpy is synchronous on default stream**: No need for cudaDeviceSynchronize after timing — the function blocks until the copy completes.
- **--skip-nvme produces garbage data in buffers**: This is fine for timing measurement. The buffers are allocated but contain zeros/stale data. The cudaMemcpy still copies real bytes at full speed regardless of content.
- **Timing resolution**: `Instant::now()` on Linux uses `clock_gettime(CLOCK_MONOTONIC)` with nanosecond precision. Microsecond reporting is appropriate for operations in the 100us-2000us range.
