# Handoff — Iteration 2: GPU Sync Stall Elimination

## Goal

Measure the throughput impact of removing or relaxing the periodic GPU stream synchronization (`stream_synchronize` every 16 H2D copies) in the sliding-window pipeline. Build the server with `--features p2p`, run benchmarks, and compare throughput/latency across 3 conditions: sync removal alone (h-main), sync removal + QD64/4-streams (h-ablation), and relaxed sync interval to 64 (h-robustness).

## Key Discoveries

- **Baseline confirmed at 3.87 GB/s**: Two consecutive runs on the current unmodified code with 2 drives yielded 3.91 and 3.83 GB/s cold lookup (mean 3.87). This matches iteration 1's baseline of 3.93 GB/s within variance.
- **GPU sync blocks NVMe processing**: The sync at `pipeline.rs:381` calls `stream_synchronize` on BOTH streams, blocking the thread that processes NVMe completions. During this block (~167 us for 2 MiB at 12 GB/s H2D), the NVMe command queue drains because no completions are consumed and no new reads submitted.
- **Sync is safe to remove**: Each chunk targets a unique memory segment. GPU copies only start after NVMe completion is confirmed (line 317-340). The sync exists solely to bound GPU command queue depth, not prevent data races. CUDA handles internal queue management.
- **stream_idx is function-local**: At `pipeline.rs:310`, `let mut stream_idx = 0usize;` — resets every call to `pipelined_ssd_to_gpu_zero_copy()`. So `% 64` with 32 chunks means NO sync fires within a single 4 MiB object.
- **2 drives required for ~4 GB/s baseline**: Single-drive baseline is only 2.27 GB/s. The benchmark distributes objects across drives (16 objects, 2 drives = 8 per drive).
- **The batch_lookup path creates per-thread streams**: At `lib.rs:1111-1116`, each thread creates its own 2-stream array. When expanding to 4 streams, this code must also be updated.
- **Single-drive ceiling is ~5.4 GB/s**: With 2 drives and objects distributed evenly, the effective per-drive load is halved, so the per-drive NVMe queue depth matters less. The overall system ceiling is ~10.8 GB/s (2 × 5.4).

## System Interface

- **Build:** `cargo build -p certus-server --release --features p2p`
- **Run baseline:** `./target/release/certus-server --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 --format --poller-base-cpu 2`
- **Benchmark:** `python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
- **Output format:** Stdout, look for `Lookup (cold)` section then `per-client=X.XX GB/s` and `p99=XXXXX.X us`
- **Baseline result:** 3.87 GB/s cold lookup (mean of 2 runs)
- **Server wait:** Sleep 6s after starting server (SPDK + CUDA init)
- **Kill server:** `kill $(ps -ef | grep "certus-server" | grep -v grep | grep -v bash | awk '{print $2}')`
- **Integrity check:** `python3 apps/python/certus-api-bench.py --verify-integrity`

## Code Map

| File:Line | What | When to check |
|-----------|------|---------------|
| `components/dispatcher/src/pipeline.rs:18` | `PIPELINE_RING_SIZE = 8` | Ring buffer count (unchanged this iter) |
| `components/dispatcher/src/pipeline.rs:29` | `streams: [GpuStream; 2]` | h-ablation needs [GpuStream; 4] |
| `components/dispatcher/src/pipeline.rs:46-52` | Stream creation in PipelineRing::new() | h-ablation needs 4 streams created |
| `components/dispatcher/src/pipeline.rs:62-65` | destroy() destroys 2 streams | h-ablation needs to destroy 4 |
| `components/dispatcher/src/pipeline.rs:244` | `pipelined_ssd_to_gpu_zero_copy()` | Active cold-path function |
| `components/dispatcher/src/pipeline.rs:310` | `let mut stream_idx = 0usize;` | Confirms stream_idx is function-local |
| `components/dispatcher/src/pipeline.rs:365` | `streams[stream_idx % 2]` | h-ablation changes to `% 4` |
| `components/dispatcher/src/pipeline.rs:381-386` | `if stream_idx % 16 == 0 { sync both streams }` | **PRIMARY TARGET** — remove or change interval |
| `components/dispatcher/src/pipeline.rs:389-393` | Final sync (both streams) | KEEP — ensures all copies done before return |
| `components/dispatcher/src/lib.rs:271` | `max_queue_depth = 16` in promote_and_serve | h-ablation changes to 64 |
| `components/dispatcher/src/lib.rs:1046` | `MAX_QUEUES_PER_DRIVE = 2` | h-ablation changes to 4 |
| `components/dispatcher/src/lib.rs:1091` | `queue_depth = 16 / num_queues` | h-ablation changes to `64 / num_queues` |
| `components/dispatcher/src/lib.rs:1111-1116` | Per-thread stream creation in batch_lookup | h-ablation needs 4 streams |
| `components/block-device-spdk-nvme/src/controller.rs:158` | `max_transfer_size = 131072` | Keep at 128 KiB (RP-2) |

## Code Targets

### h-main (Remove mid-transfer sync)
- `components/dispatcher/src/pipeline.rs:381-386` — **DELETE** the entire `if stream_idx % 16 == 0 { ... }` block (6 lines). This is the only change.

### h-ablation (QD64 + 4 streams + sync removal)
All h-main changes PLUS:
- `components/dispatcher/src/pipeline.rs:29` — change `[GpuStream; 2]` to `[GpuStream; 4]`
- `components/dispatcher/src/pipeline.rs:46-58` — create 4 streams in `new()` (add stream_c, stream_d)
- `components/dispatcher/src/pipeline.rs:62-65` — destroy all 4 streams in `destroy()`
- `components/dispatcher/src/pipeline.rs:247` — change function parameter `streams: &[GpuStream; 2]` to `&[GpuStream; 4]`
- `components/dispatcher/src/pipeline.rs:365` — change `streams[stream_idx % 2]` to `streams[stream_idx % 4]`
- `components/dispatcher/src/pipeline.rs:389-393` — iterate over all 4 streams for final sync
- `components/dispatcher/src/lib.rs:271` — change `16` to `64` (max_queue_depth)
- `components/dispatcher/src/lib.rs:1046` — change `MAX_QUEUES_PER_DRIVE` from 2 to 4
- `components/dispatcher/src/lib.rs:1091` — change to `64 / num_queues`
- `components/dispatcher/src/lib.rs:1111-1116` — create 4 streams per thread (currently creates 2)

### h-robustness (Sync interval 16→64)
- `components/dispatcher/src/pipeline.rs:381` — change `% 16` to `% 64`. Single-line change.

## What I Tried That Didn't Work

- **Single-drive baseline gives only 2.27 GB/s**: Must use 2 drives (0000:61:00.0 and 0000:62:00.0) to match iteration 1's baseline of ~3.9 GB/s. The benchmark distributes 16 objects across drives.
- **Exit code 144 when running bash commands with backgrounded server + kill**: The shell propagates SIGCHLD from the killed server. Use separate commands for kill, start, and benchmark rather than chaining them.

## What I Excluded and Why

- **True GPU P2P (NVMe→GPU via GDRCopy BAR mapping)**: The `create_spdk_dma_buffer_from_gpu_bar()` function exists in `dma.rs:353` and can DMA NVMe directly to GPU BAR1. Excluded because: (1) The cold path MUST populate the memory-tier for caching — skipping DRAM means no hot-path cache hits, (2) Would require a parallel-write architecture (NVMe→DRAM for cache + NVMe→GPU for immediate use), doubling NVMe bandwidth usage, (3) The current bottleneck appears to be GPU sync stalls, not the H2D copy itself (PCIe Gen4 x16 = 25.6 GB/s >> 4 GB/s needed).
- **Smaller chunks (64 KiB)**: More chunks (64 per object) would mean more pipeline segments but smaller NVMe commands. The overhead of 64 smaller reads may outweigh the parallelism benefit. Better to first address the sync stall with existing 128 KiB chunks.
- **Multi-client benchmarks**: Iteration 1 established single-client performance. Multi-client testing is a separate dimension that introduces contention variables.
- **CPU core affinity changes**: `--poller-base-cpu 2` is used in all conditions. Not a variable.

## Evolution of Thinking

Initially considered true GPU P2P as the iteration 2 focus (as suggested in iter-1 handoff). After reading the code, realized:
1. The H2D copy is NOT the bottleneck — at 12 GB/s real-world PCIe bandwidth, copying 4 MiB takes only 333 us, which is less than the NVMe read time.
2. The real bottleneck is the GPU **sync** — not the copy itself. The sync blocks the entire pipeline loop, creating dead time where NVMe completions go unprocessed.
3. The sync at line 381 is a performance-only throttle (bounds GPU command queue), not a correctness requirement. Each chunk uses independent memory, and GPU copies only start after NVMe completion is confirmed.
4. True GPU P2P would add complexity without addressing the actual bottleneck, and would break the hot-path caching model.

The key insight: with 32 chunks per object and sync every 16, exactly one sync fires mid-object. This creates a deterministic bubble every ~half object transfer. Eliminating it should yield a clean throughput improvement.

## Current Status

- **Validated:** Build succeeds with `--features p2p`. Server starts on port 50051 with 2 drives. Baseline cold lookup = 3.87 GB/s. Code paths verified — sync is at line 381, stream_idx is local, each chunk uses independent memory.
- **Uncertain:** (1) Exact GPU sync stall duration — estimated at 167 us based on 2 MiB at 12 GB/s, but CUDA may batch copies internally making sync faster. (2) Whether CUDA runtime internally throttles when too many async copies are queued without sync — if so, removing sync may not help. (3) How much the stall actually drains the NVMe queue — depends on NVMe completion latency vs sync duration.
- **Suggested next:** If sync removal shows improvement, iteration 3 should explore: (1) Optimal sync interval as a function of object size (parameterize rather than hardcode), (2) Overlapping multiple objects' transfers (pipeline across objects, not just within), (3) Smaller chunk sizes (64 KiB) with QD128 for maximum pipeline depth, now that GPU sync is no longer a bottleneck.

## Warnings & Constraints

- **Server must be killed and restarted after code changes** — SPDK binds NVMe at startup.
- **Sleep 6s after server start** — SPDK + CUDA + memory-tier registration takes 3-5s.
- **Use 2 drives for benchmark** — Single drive gives only 2.27 GB/s baseline (vs 3.87 with 2 drives).
- **h-ablation has many code changes** — The 4-stream expansion touches struct definition, constructor, destructor, function signature, and all callers. Verify compilation carefully.
- **The `pipelined_ssd_to_gpu_zero_copy` function signature has `streams: &[GpuStream; 2]`** — h-ablation must also change this to `&[GpuStream; 4]` AND update all call sites (lib.rs:261 and lib.rs:1164).
- **batch_lookup streams at lib.rs:1111-1116** creates a local 2-element array — must be expanded to 4 for h-ablation, AND the pipeline call at line 1164 must pass a 4-element reference.
- **The PipelineRing test at pipeline.rs:408-412 asserts size <= 16** — update this bound if PIPELINE_RING_SIZE changes (not needed this iteration since ring size stays at 8).
