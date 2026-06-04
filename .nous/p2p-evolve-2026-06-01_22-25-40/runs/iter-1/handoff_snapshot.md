# Handoff — Iteration 1: Pipeline Parallelism

## Goal

Measure the throughput impact of increasing NVMe queue depth (16→64), CUDA stream count (2→4), and chunk size (128 KiB→512 KiB) on the cold lookup path. Build the server with `--features p2p`, run benchmarks, and compare throughput/latency across 4 conditions (baseline, QD+streams, chunk size, combined).

## Key Discoveries

- **Zero-copy pipeline is the active cold path**: `pipelined_ssd_to_gpu_zero_copy()` at `pipeline.rs:244` reads directly into the memory-tier slot (no intermediate ring copy). The original `pipelined_ssd_to_gpu()` at line 85 is the ring-based fallback.
- **Queue depth is passed as a parameter, not a constant**: `promote_and_serve()` passes `16` hardcoded at `lib.rs:271`. The `batch_lookup()` path calculates `queue_depth = 16 / num_queues` at line 1091, where `num_queues = min(MAX_QUEUES_PER_DRIVE=2, entry_count)`.
- **Stream sync every 16 GPU copies**: At `pipeline.rs:381`, a full sync of both streams triggers every 16 completed H2D copies. This is the GPU-side throttle point.
- **max_transfer_size = 128 KiB is a code default, not hardware-reported**: `controller.rs:158` sets it to 131072 as a constant. The actual NVMe MDTS may be larger — changing this is safe as long as the drive supports it (Gen4 drives typically support 1 MB+).
- **Memory-tier pool is CUDA-pinned + SPDK-registered at startup**: `lib.rs:716-733` registers the pool for zero-copy DMA. The zero-copy path works because NVMe can DMA directly into CUDA-pinned host memory.
- **Batch lookup spawns scoped threads per drive with per-thread queue pairs**: `lib.rs:1075-1212` — each thread gets its own NVMe `connect_client()` (queue pair) and 2 CUDA streams, enabling concurrent reads.
- **4 MiB objects / 128 KiB chunks = 32 NVMe commands per object**: At QD16, only half the object's chunks can be in-flight simultaneously.

## System Interface

- **Build:** `cargo build -p certus-server --release --features p2p`
- **Run baseline:** `./target/release/certus-server --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 --format --poller-base-cpu 2`
- **Benchmark:** `python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
- **Output format:** Stdout, look for `Lookup (cold)` section then `per-client=X.XX GB/s` and `p99=XXXXX.X us`
- **Baseline result:** ~2.4 GB/s cold lookup per prior measurements
- **Server wait:** Sleep 5s after starting server (SPDK init takes ~3s)
- **Kill server:** `pkill -f certus-server` (must restart after code changes)
- **Integrity check:** Add `--verify-integrity` to benchmark for data correctness

## Code Map

| File:Line | What | When to check |
|-----------|------|---------------|
| `components/dispatcher/src/pipeline.rs:18` | `PIPELINE_RING_SIZE = 8` | Modifying ring buffer depth |
| `components/dispatcher/src/pipeline.rs:29` | `streams: [GpuStream; 2]` | Changing CUDA stream count |
| `components/dispatcher/src/pipeline.rs:244` | `pipelined_ssd_to_gpu_zero_copy()` | The active cold-path function |
| `components/dispatcher/src/pipeline.rs:381` | `if stream_idx % 16 == 0` | GPU sync interval — throttle point |
| `components/dispatcher/src/lib.rs:271` | `max_queue_depth = 16` | Single-lookup queue depth param |
| `components/dispatcher/src/lib.rs:1046` | `MAX_QUEUES_PER_DRIVE = 2` | Batch-lookup parallel queue threads |
| `components/dispatcher/src/lib.rs:1091` | `queue_depth = 16 / num_queues` | Batch-lookup per-thread QD |
| `components/block-device-spdk-nvme/src/controller.rs:158` | `max_transfer_size: 131072` | Chunk size for NVMe commands |
| `apps/certus-server/src/main.rs:246` | `DispatcherConfig { ... }` | Server init config |
| `apps/python/certus-api-bench.py:630` | `main()` argument parsing | Benchmark CLI flags |

## Code Targets

### h-main (QD64 + 4 streams)
- `components/dispatcher/src/pipeline.rs:18` — change `PIPELINE_RING_SIZE` from 8 to 32
- `components/dispatcher/src/pipeline.rs:29` — expand `streams` to `[GpuStream; 4]`, create 4 streams in `new()` (lines 46-58)
- `components/dispatcher/src/pipeline.rs:381` — change `% 16` to `% 32`
- `components/dispatcher/src/lib.rs:271` — change `16` to `64` (max_queue_depth in promote_and_serve)
- `components/dispatcher/src/lib.rs:1046` — change `MAX_QUEUES_PER_DRIVE` from 2 to 4
- `components/dispatcher/src/lib.rs:1091` — change denominator to compute `64 / num_queues`

### h-ablation (512 KiB chunks)
- `components/block-device-spdk-nvme/src/controller.rs:158` — change `131072` to `524288`

### h-super-additivity (combined)
- All changes from h-main + h-ablation applied together

## What I Tried That Didn't Work

- Nothing failed in exploration. All file paths and line numbers verified via direct reads.

## What I Excluded and Why

- **True GPU P2P (NVMe→GPU direct via GDRCopy BAR mapping):** The `p2p` feature enables `create_spdk_dma_buffer_from_gpu_bar()` (dma.rs:353) for NVMe→GPU direct DMA. Excluded because: (1) the current server's cold-path requires writing to the memory-tier (for caching), so bypassing DRAM would skip cache population; (2) requires significant architectural changes to the dispatch model; (3) better suited for iteration 2 after establishing pipeline parallelism baselines.
- **Multiple drives:** The benchmark targets single-drive throughput to isolate pipeline efficiency. Multi-drive scaling is a separate dimension.
- **Hot-path optimization:** Memory-tier→GPU path is already fast (limited by PCIe bandwidth, not pipeline design). Cold path is the bottleneck.
- **CPU core pinning:** The `--poller-base-cpu` flag already exists and will be used in all conditions. Not a variable in this experiment.

## Evolution of Thinking

Initially considered testing the full P2P (NVMe→GPU direct) path as the primary hypothesis, but after reading the code realized: (1) the zero-copy pipeline already eliminates one memcpy (reads into memory-tier directly, no ring bounce), and (2) the `p2p` feature's BAR mapping functions are designed for cross-process use (the `gpu-p2p-server` binary), not the in-process dispatcher. The real bottleneck is NVMe queue depth and GPU DMA scheduling, not the DRAM→GPU copy (which is PCIe Gen4 x16 = 25+ GB/s and not limiting).

## Current Status

- **Validated:** File paths, line numbers, code structure, feature flags, CLI arguments
- **Uncertain:** Actual MDTS of the target NVMe drive (code uses 128 KiB default, drive likely supports more). If 512 KiB exceeds MDTS, SPDK will reject the command — the executor should check for I/O errors.
- **Suggested next:** If this iteration shows QD64 saturates the single drive, iteration 2 should explore: (1) true GPU P2P bypassing DRAM for latency-sensitive paths, (2) multi-stream NVMe (multiple queue pairs per object), (3) adaptive chunk sizing based on object size

## Warnings & Constraints

- **Server must be killed and restarted after code changes** — SPDK binds to NVMe devices at startup and cannot rebind.
- **Sleep 5s after server start** — SPDK init + CUDA init + memory-tier registration takes ~3-5s. Port 50051 won't be ready before then.
- **max_transfer_size is not queried from hardware** — it's hardcoded at 131072. If increased beyond the drive's actual MDTS, NVMe commands will fail with an I/O error. Check server logs if cold lookups fail after changing chunk size.
- **The benchmark's `--block-size` is the object size (4 MiB), not the NVMe chunk size** — don't confuse these.
- **PipelineRing's streams array size is a const generic** — changing from `[GpuStream; 2]` to `[GpuStream; 4]` requires updating the struct definition, all places that index into it (the `% 2` patterns become `% 4`), and the destroy logic.
