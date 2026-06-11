---
name: profile-performance-breakdown
description: Run the cold-lookup pipeline telemetry breakdown showing where time is spent (NVMe wait, GPU DMA, stream sync)
argument-hint: "[--drives N] [--block-size 4M] [--iterations 20] [--clients 1]"
---

Run a performance breakdown of the cold-lookup pipeline using the `pipeline-telemetry` feature flag. This shows exactly where time is spent during SSD→DRAM→GPU promotions: NVMe read wait, GPU DMA issue, stream synchronization, etc.

## Steps

1. Build certus-server with telemetry enabled:
   ```bash
   cargo build --release -p certus-server --features dispatcher/pipeline-telemetry
   ```

2. Start certus-server. Parse arguments for drive count (default 4), memory-tier size (default 8G):
   ```bash
   target/release/certus-server --drive-count <N> --format --memory-tier-size 8589934592 > /tmp/certus-telemetry.log 2>&1 &
   ```
   Wait for "listening" in the log before proceeding.

3. Run the v2 benchmark to generate cold-lookup traffic:
   ```bash
   python3 apps/python/certus-api-bench_v2.py \
     --server localhost:50051 \
     --clients <clients, default 1> \
     --gpus 1 \
     --num-objects 16 \
     --iterations <iterations, default 20> \
     --block-size <block-size, default 4M> \
     --batch-size 16 \
     --writes-settle 0 \
     --pipeline-depth 4
   ```

4. Stop the server:
   ```bash
   killall certus-server
   ```

5. Parse the telemetry output from the server log and present two tables:

   **Pipeline Stage Breakdown** — parse `[pipeline-perf]` lines:
   - Extract: `submit`, `recv_wait`, `gpu_dma`, `sync`, `resub`, `final_sync` (all in ms)
   - Compute: average across all calls, percentage of total

   **Cold-Path Overhead** — parse `[cold-perf]` lines:
   - Extract: `drive`, `jobs`, `prep`, `pipeline`, `finalize` (all in ms)
   - Compute: per-drive average pipeline time

6. Present results in a markdown table with:
   - Stage name, average time (ms), percentage of total, description
   - Per-drive breakdown showing distribution balance
   - Identify the dominant bottleneck (typically `sync` = GPU stream_synchronize)

7. Rebuild certus-server WITHOUT telemetry to restore normal performance (telemetry adds measurement overhead):
   ```bash
   cargo build --release -p certus-server
   ```

## Interpreting Results

- **recv_wait** — Time waiting for NVMe read completions. High values indicate SSD latency or NVMe queue saturation.
- **gpu_dma** — Time issuing `dma_copy_to_device_async` calls. Should be near-zero (just queueing).
- **sync** — Time in `stream_synchronize` waiting for GPU H2D DMA to complete. Dominates when PCIe link is the bottleneck (expected with multiple drives sharing one GPU).
- **resub** — Time resubmitting next NVMe reads after completions. Should be near-zero.
- **prep** — Eviction + memory-tier allocation. High values indicate memory-tier lock contention.
- **finalize** — Dispatch-map state updates after promotion. Should be near-zero.

## Notes

- The `pipeline-telemetry` feature uses `eprintln!` and `std::time::Instant`, adding ~1-5% overhead to cold-path latency. Do not use for production benchmarking.
- Results vary with drive count, block size, and concurrency. Run with the same parameters as your production workload for relevant data.
