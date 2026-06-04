# Handoff — Iteration 3: Batch Path Queue Depth and Thread Parallelism

## Goal

Apply the QD64 + 4 streams optimization (proven in iter-1/2 on the single-object path) to the **batch_lookup** code path that the benchmark actually exercises. The batch path currently runs at QD8 per thread (16 total budget ÷ 2 threads), which is the actual bottleneck measured in baseline benchmarks. Increase queue depth budget to 128 and thread count to 4, then measure throughput across 3 conditions.

## Key Discoveries

- **The benchmark exercises batch_lookup, not promote_and_serve**: Cold lookups use `BatchLookupRequest` with 16 entries → `batch_lookup()` at `lib.rs:886`. This goes through the parallel per-drive thread path at `lib.rs:1075`, NOT the single-object `promote_and_serve` at `lib.rs:194`. The iter-1/2 improvements (QD64 on the single-object path at line 271) never changed the batch path's QD8.
- **Batch path queue depth is QD8**: `queue_depth = 16 / num_queues` (`lib.rs:1091`) with `MAX_QUEUES_PER_DRIVE=2` gives each thread only 8 in-flight NVMe commands. With 32 chunks per 4 MiB object (at 128 KiB each), only 25% of the pipeline is filled at any time.
- **Actor selects queue pairs by pending_ops count**: At `actor.rs:591`, the NVMe actor picks queue pairs via `select_index(pending_ops.len() + 1)`. With QD8 per thread, it uses the depth-16 queue pair. With QD32+ per thread, it uses depth-64 or depth-256, enabling the NVMe controller's internal scheduling.
- **QueuePairPool has a depth-256 pair**: `qpair.rs:141` shows `STANDARD_DEPTHS = [4, 16, 64, 256]`. All four are allocated at startup. The deep pairs exist but are unused at low queue depths.
- **Each connect_client creates channels, not hardware queue pairs**: Multiple threads calling `connect_client` (`lib.rs:1105`) get separate SPSC channels to the actor. The actor multiplexes across hardware queue pairs. No per-thread hardware queue allocation limit.
- **Baseline confirmed at 3.89 GB/s**: Verified on 2026-06-01 with unmodified code, 2 drives, matching iter-2's 3.87 GB/s.
- **drive_index is `key % num_drives`** (`lib.rs:107`): With 16 sequential keys and 2 drives, objects distribute evenly (8 per drive). Single-drive test puts all 16 on one device.

## System Interface

- **Build:** `cargo build -p certus-server --release --features p2p`
- **Run baseline (2 drives):** `./target/release/certus-server --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 --format --poller-base-cpu 2`
- **Run single drive:** `./target/release/certus-server --device-pci 0000:61:00.0 --format --poller-base-cpu 2`
- **Benchmark:** `python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
- **Output format:** Stdout, look for `Lookup (cold)` then `per-client=X.XX GB/s` and `p99=XXXXX.X us`
- **Baseline result:** 3.89 GB/s cold lookup (2 drives), 2.27 GB/s (1 drive, from iter-2)
- **Server wait:** Sleep 6s after starting server (SPDK + CUDA init)
- **Kill server:** `kill $(ps -ef | grep "certus-server" | grep -v grep | grep -v bash | awk '{print $2}')`

## Code Map

| File:Line | What | When to check |
|-----------|------|---------------|
| `components/dispatcher/src/lib.rs:1046` | `MAX_QUEUES_PER_DRIVE = 2` | **PRIMARY TARGET** — h-main changes to 4 |
| `components/dispatcher/src/lib.rs:1091` | `queue_depth = 16 / num_queues` | **PRIMARY TARGET** — change 16 to 128 |
| `components/dispatcher/src/lib.rs:1111-1116` | Per-thread stream creation (2 streams) | h-main creates 4 streams here |
| `components/dispatcher/src/lib.rs:1195-1196` | Per-thread stream destruction | h-main destroys 4 streams |
| `components/dispatcher/src/lib.rs:1164` | Pipeline call in batch thread — passes `&streams` | Must match function signature |
| `components/dispatcher/src/lib.rs:271` | Single-object path QD=16 | Change to 64 when function signature requires [GpuStream; 4] |
| `components/dispatcher/src/lib.rs:264` | Single-object path passes `&ring_ref.streams` | Must match [GpuStream; 4] after h-main changes |
| `components/dispatcher/src/pipeline.rs:29` | `pub streams: [GpuStream; 2]` | h-main changes to [GpuStream; 4] |
| `components/dispatcher/src/pipeline.rs:46-52` | PipelineRing::new() creates 2 streams | h-main creates 4 |
| `components/dispatcher/src/pipeline.rs:62-65` | PipelineRing::destroy() destroys 2 | h-main destroys 4 |
| `components/dispatcher/src/pipeline.rs:247` | `streams: &[GpuStream; 2]` function param | h-main changes to `&[GpuStream; 4]` |
| `components/dispatcher/src/pipeline.rs:365` | `streams[stream_idx % 2]` | h-main changes to `% 4` |
| `components/dispatcher/src/pipeline.rs:381-386` | Mid-transfer sync block | DELETE in all arms |
| `components/dispatcher/src/pipeline.rs:389-393` | Final sync iterates over streams slice | h-main: ensure it covers all 4 |
| `components/block-device-spdk-nvme/src/qpair.rs:141` | `STANDARD_DEPTHS = [4, 16, 64, 256]` | Verify depth-256 exists for high aggregate QD |
| `components/block-device-spdk-nvme/src/actor.rs:591` | Queue pair selection by pending_ops | Understanding only — no changes |

## Code Targets

### h-main (QD32 × 4 threads × 4 streams)

Changes to `components/dispatcher/src/lib.rs`:
- Line 1046: `MAX_QUEUES_PER_DRIVE` 2→4
- Line 1091: `16 / num_queues` → `128 / num_queues`
- Lines 1111-1116: Create 4 streams (add 2 more create_stream calls, build a `[a, b, c, d]` array)
- Lines 1195-1196: Destroy 4 streams (currently destroys `streams[0]` and `streams[1]`, add `streams[2]` and `streams[3]`)
- Line 271: Change 16 to 64 (single-object path QD)

Changes to `components/dispatcher/src/pipeline.rs`:
- Line 29: `[GpuStream; 2]` → `[GpuStream; 4]`
- Lines 46-58: Create 4 streams in `PipelineRing::new()` (chain `create_stream` calls for c and d)
- Lines 62-65: Destroy all 4 in `destroy()`
- Line 247: Function param `&[GpuStream; 2]` → `&[GpuStream; 4]`
- Line 365: `stream_idx % 2` → `stream_idx % 4`
- Lines 381-386: DELETE entire `if stream_idx % 16 == 0 { ... }` block
- Lines 389-393: Final sync already iterates the slice (`for s in streams`), so it naturally covers 4 elements if the type is `&[GpuStream; 4]`. Verify this compiles.

### h-ablation (QD64 × 2 threads × 2 streams)

Changes to `components/dispatcher/src/lib.rs`:
- Line 1091: `16 / num_queues` → `128 / num_queues` (with MAX_QUEUES=2, this gives 64)
- Line 271: 16 → 64

Changes to `components/dispatcher/src/pipeline.rs`:
- Lines 381-386: DELETE mid-transfer sync block

**DO NOT** change PipelineRing struct, function signature, stream count, or MAX_QUEUES_PER_DRIVE. The `[GpuStream; 2]` signature stays.

### h-robustness (Same code as h-main, different runtime config)

Same code changes as h-main. Different server command:
```bash
./target/release/certus-server --device-pci 0000:61:00.0 --format --poller-base-cpu 2
```
(Single drive only.)

## What I Tried That Didn't Work

- **Exit code 144 when backgrounding server + kill in same command**: Use separate bash calls for start, benchmark, and kill (from iter-2, still applies).
- **GPU sync removal alone (RP-4/5)**: No throughput impact at any queue depth tested. Not worth investigating further.
- **512 KiB chunks (RP-2)**: Reduces throughput by 5%. Keep chunks at 128 KiB.
- **Single drive gives only 2.27 GB/s baseline**: Must use 2 drives for h-main and h-ablation to compare against the 3.89 GB/s baseline.

## What I Excluded and Why

- **True GPU P2P (NVMe→GPU via BAR mapping)**: Excluded in iter-2 because it bypasses the memory-tier cache, breaking the hot-path model. Still excluded.
- **Inter-object pipeline overlap within a single thread**: Could pipeline the GPU copies of object N with NVMe reads of object N+1 within the same thread. Excluded because adding more threads (h-main) achieves the same overlap more simply — with 4 threads, 4 objects are naturally concurrent.
- **Larger ring sizes (PIPELINE_RING_SIZE > 8)**: The zero-copy path doesn't use the ring buffer at all — it creates DmaBuffer wrappers over the memory-tier slot directly. The `PipelineRing` is only used for the non-zero-copy fallback. Ring size is irrelevant to the benchmark path.
- **Multi-client benchmarks**: Adds contention complexity. Single-client with 16 objects is enough to test batch path parallelism.
- **queue_depth > 64 per thread**: RP-3 warned about ENOMEM with deep queues + large chunks. While 128 KiB should be safe, going to QD128+ per thread is unnecessary when 4 threads × QD32 already provides aggregate QD128 per drive.

## Evolution of Thinking

Iteration 2 focused on GPU sync stalls, proving they're not the bottleneck (RP-4/5). The dominant factor was confirmed as NVMe queue depth (RP-1). But the insight I discovered is that the iter-1/2 QD64 improvements were only applied to the **single-object path** (`promote_and_serve` line 271), which is NOT exercised by the benchmark.

The benchmark uses `BatchLookupRequest` → `batch_lookup()` → parallel threads with `queue_depth = 16 / num_queues`. The actual benchmark has been running at QD8 all along! This explains why the baseline is only 3.89 GB/s despite iter-1 proving QD64 gives 4.5+ GB/s — those experiments must have modified the batch path directly (not preserved in the current codebase on the evolve-p2p branch).

This iteration is essentially applying RP-1 to the correct code path for the first time, plus testing whether 4× thread parallelism provides additional benefit beyond deeper per-thread queues.

## Current Status

- **Validated:** Build succeeds with `--features p2p`. Server starts on port 50051 with 2 drives. Baseline cold lookup = 3.89 GB/s. Batch path confirmed as the exercised code path. QD8 confirmed as current per-thread queue depth.
- **Uncertain:** (1) Whether 4 threads × QD32 contend on the single-actor poll loop — the actor processes all threads' commands on one OS thread. (2) Whether memory-tier eviction (`evict_for_space` at lib.rs:1149) serializes between 4 threads — it takes `&Arc<dyn IMemoryTier>` so likely contends on an internal lock. (3) Whether the h-ablation (QD64, 2 threads) will match h-main — if so, thread count doesn't matter.
- **Suggested next:** If h-main confirms >4.5 GB/s, iteration 4 should explore: (1) Scaling to 4+ clients to test contention, (2) Whether 8+ objects per batch further improves throughput (currently 8 per drive with 16 objects), (3) Whether the single-object path at line 271 (used by non-batch lookups) should also get the full QD64+4 streams treatment permanently.

## Warnings & Constraints

- **h-main has many code changes** — The 4-stream expansion touches the PipelineRing struct, its constructor, destructor, the zero-copy function signature, the batch thread's stream creation, and all callers. The h-ablation is simpler (just QD change + sync removal).
- **h-ablation MUST NOT change the function signature** — It keeps `&[GpuStream; 2]`. Only change the QD at lib.rs:1091 and remove the sync. The PipelineRing struct stays at 2 streams.
- **h-robustness uses the same binary as h-main** — Only the server CLI flags differ (1 drive vs 2). Build once, test twice.
- **Server must be killed between conditions** — SPDK binds NVMe at startup, no hot-reload.
- **Sleep 6s after server start** — SPDK + CUDA + memory-tier registration needs 3-5s.
- **Monitor stderr for ENOMEM/rc=-12** — Especially in h-robustness where all 16 objects hit one drive. If it appears, the condition fails but the data is still valuable (confirms RP-3 boundary).
- **The `pipelined_ssd_to_gpu` function (non-zero-copy, line 85)** is NOT exercised by the cold lookup benchmark. Only `pipelined_ssd_to_gpu_zero_copy` (line 244) is called. Changes to `pipelined_ssd_to_gpu` are unnecessary but the PipelineRing struct serves both paths, so struct changes affect both.
