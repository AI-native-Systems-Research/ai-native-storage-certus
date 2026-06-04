# Problem Framing — Iteration 3: Batch Path Queue Depth and Thread Parallelism

## Research Question

The benchmark's cold lookup path exercises `batch_lookup()` (`lib.rs:886`), which spawns per-drive worker threads with `queue_depth = 16 / num_queues` (`lib.rs:1091`). With `MAX_QUEUES_PER_DRIVE=2` (`lib.rs:1046`), each thread operates at QD8 — far below the QD64 that iteration 1 proved optimal for single-object transfers. Can increasing the batch path's queue depth budget and thread count recover the 21% throughput gain (RP-1) that was demonstrated on the single-object path but never applied to the actual benchmark code path?

The hypothesis is that the batch path is bottlenecked by shallow per-thread queue depth (QD8), not by GPU transfer overhead or drive bandwidth. Increasing per-thread queue depth allows more NVMe commands in flight, better saturating the device's internal parallelism.

## System Interface

- **Build:** `cargo build -p certus-server --release --features p2p`
- **Server start:** `./target/release/certus-server --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 --format --poller-base-cpu 2`
- **Benchmark:** `python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
- **Output format:** Stdout. Cold lookup throughput at `Lookup (cold)` → `per-client=X.XX GB/s`. P99 latency at `p99=XXXXX.X us`.
- **Server wait:** 6 seconds after start for SPDK + CUDA initialization.
- **Kill server:** `kill $(ps -ef | grep "certus-server" | grep -v grep | grep -v bash | awk '{print $2}')`

### Code Evidence

| Flag/Parameter | Location | Current Value |
|---|---|---|
| MAX_QUEUES_PER_DRIVE | `components/dispatcher/src/lib.rs:1046` | 2 |
| queue_depth formula | `components/dispatcher/src/lib.rs:1091` | `16 / num_queues` → 8 per thread |
| Per-thread stream count | `components/dispatcher/src/lib.rs:1111-1116` | 2 streams per thread |
| Pipeline function streams param | `components/dispatcher/src/pipeline.rs:247` | `&[GpuStream; 2]` |
| Stream rotation | `components/dispatcher/src/pipeline.rs:365` | `streams[stream_idx % 2]` |
| Mid-transfer sync | `components/dispatcher/src/pipeline.rs:381` | Every 16 H2D copies |
| Chunk size (MDTS) | `components/dispatcher/src/lib.rs:687-691` | 131072 (128 KiB) |
| QueuePairPool depths | `components/block-device-spdk-nvme/src/qpair.rs:141` | [4, 16, 64, 256] |
| Queue pair selection | `components/block-device-spdk-nvme/src/actor.rs:591` | `select_index(pending_ops.len() + 1)` |
| Single-object path QD | `components/dispatcher/src/lib.rs:271` | 16 |

## Baseline Command

```bash
./target/release/certus-server --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 --format --poller-base-cpu 2 &
sleep 6
python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304
kill $(ps -ef | grep "certus-server" | grep -v grep | grep -v bash | awk '{print $2}')
```

## Baseline Validation

Exit code 0. Output includes:
```
Lookup (cold)        avg=   1077.8 us  p50=   1068.5 us  p99=   1258.4 us  min=   1010.3 us  max=   1258.4 us
                     per-client=  3.89 GB/s  aggregate=  3.88 GB/s
```

Confirmed: 3.89 GB/s cold lookup (consistent with iter-2 baseline of 3.87 GB/s).

## Experimental Conditions

### h-main: Deep Queues + 4 Threads (QD32 per thread)

Increase total queue depth budget from 16 to 128 AND expand MAX_QUEUES_PER_DRIVE from 2 to 4. This gives each thread QD32 with 4 concurrent threads per drive. Also expand per-thread streams from 2 to 4 and remove mid-transfer sync (which RP-4/RP-5 showed is not the bottleneck but adds unnecessary overhead at deep queue depths).

Code changes:
- `lib.rs:1046` — change `MAX_QUEUES_PER_DRIVE` from 2 to 4
- `lib.rs:1091` — change `16 / num_queues` to `128 / num_queues` (→ 32 per thread)
- `lib.rs:1111-1116` — create 4 streams per thread instead of 2
- `pipeline.rs:247` — change function signature from `&[GpuStream; 2]` to `&[GpuStream; 4]`
- `pipeline.rs:365` — change `streams[stream_idx % 2]` to `streams[stream_idx % 4]`
- `pipeline.rs:381-386` — remove mid-transfer sync block
- `pipeline.rs:389-393` — sync all 4 streams at end
- Also update the single-object path: `lib.rs:271` change 16 to 64, and update PipelineRing struct to hold 4 streams (lines 29, 46-58, 62-65, 264)

### h-ablation: Deep Queues Only (QD64 per thread, 2 threads, 2 streams)

Increase queue depth budget from 16 to 128 but keep MAX_QUEUES_PER_DRIVE=2 and 2 streams. This gives each thread QD64 with the existing 2-thread model. Tests whether deep queues alone (without more threads/streams) capture most of the improvement.

Code changes:
- `lib.rs:1091` — change `16 / num_queues` to `128 / num_queues` (→ 64 per thread)
- `pipeline.rs:381-386` — remove mid-transfer sync (to avoid blocking at deep QD)
- `lib.rs:271` — change 16 to 64 (single-object path consistency)

### h-robustness: Single Drive with Deep Queues + 4 Threads

Same code changes as h-main, but run with only 1 NVMe drive to test whether the improvement holds when all 16 objects hit one device.

Server command:
```bash
./target/release/certus-server --device-pci 0000:61:00.0 --format --poller-base-cpu 2
```

This tests whether the deep queue + 4 threads configuration causes NVMe contention or ENOMEM failures when concentrated on a single device (RP-3 warned about this with large chunks + deep queues).

## Success Criteria

- **h-main**: Cold lookup throughput consistently above 4.5 GB/s (the iter-1 best with QD64+4 streams on the single-object path), confirming RP-1 applies to the batch path.
- **h-ablation**: Cold lookup throughput above baseline (3.89 GB/s) — quantifies how much queue depth alone contributes vs. thread parallelism.
- **h-robustness**: Cold lookup above 2.27 GB/s single-drive baseline (from iter-2 handoff) without NVMe submission failures. Tests RP-3 risk.
- **All arms**: Data integrity PASS (hard constraint), build succeeds.

## Constraints

- RP-2: Keep chunk size at 128 KiB (do NOT increase to 512 KiB — shown to reduce throughput).
- RP-3: Large chunks + deep queues risk ENOMEM. At 128 KiB chunks this should be safe (the RP-3 issue was with 512 KiB chunks), but monitor for NVMe errors.
- Server must be killed and restarted between conditions.
- Use 2 drives for h-main and h-ablation (match baseline), 1 drive for h-robustness.

## Prior Knowledge

- RP-1: QD64 + 4 streams improves throughput by ~21% over QD16 + 2 streams (4.5-4.6 GB/s vs 3.87 GB/s). This was demonstrated on the single-object path but the batch path has never been tested at these depths.
- RP-4/RP-5: GPU sync removal has negligible impact at any queue depth. It's safe to remove.
- RP-2: 128 KiB chunks are optimal (512 KiB is worse).
- RP-3: 512 KiB + QD64 causes ENOMEM. At 128 KiB this risk should be minimal.
