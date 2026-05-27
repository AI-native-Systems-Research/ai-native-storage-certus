# Baseline Measurements — 2026-05-25

Server: certus-server with 7 NVMe SSDs (running since May 22)
GPU: NVIDIA A30
Block size: 4 MiB (32 × 128 KiB chunks)
Benchmark: certus-api-bench.py --num-objects 16 --iterations 10

## SINGLE DRIVE Baseline (Bakeoff Evaluator — confirmed working)

Server: 1 data drive (0000:62:00.0)
Evaluator cycle time: ~18s (build 5s + restart 3s + bench 10s)

| Mode | Score (GB/s) | Details |
|------|-------------|---------|
| Fixed (4 MiB) | **3.59** | Single client, single drive |
| Mixed composite | **4.75** | 1 MiB=0.83, 2 MiB=2.51, 4 MiB=4.98, 16 MiB=10.68 |

**Key observation:** 1 MiB (8 chunks) = 0.83 GB/s is 6.3× worse than 16 MiB (128 chunks) = 10.68 GB/s.
This proves adaptive logic is critical — a pipeline tuned for 32 chunks is terrible at 8 chunks.

---

## 1 Client — Cold Lookup (3 runs)

| Run | Avg (µs) | p50 (µs) | p99 (µs) | Throughput (GB/s) |
|-----|----------|----------|----------|-------------------|
| 1   | 951.5    | 469.0    | 3106.2   | 4.41              |
| 2   | 785.1    | 617.9    | 1802.4   | 5.34              |
| 3   | 825.2    | 562.7    | 2683.7   | 5.08              |

**Mean: 853.9 µs, Std: 85.9 µs (10.1% CoV)**
**Mean throughput: 4.94 GB/s**

Note: High variance in avg is driven by tail latencies (p99 up to 3.1 ms). 
The p50 is more stable: 469-618 µs range. The high avg is skewed by eviction-related outliers.

## 4 Clients — Cold Lookup (3 runs)

| Run | Avg (µs) | p50 (µs) | p99 (µs) | Aggregate (GB/s) |
|-----|----------|----------|----------|-------------------|
| 1   | 2129.2   | 1936.2   | 4801.9   | 7.88              |
| 2   | 2430.2   | 2222.9   | 4995.5   | 6.90              |
| 3   | 1923.3   | 1952.5   | 4454.9   | 8.72              |

**Mean: 2160.9 µs, Mean aggregate: 7.83 GB/s**

## 8 Clients — Cold Lookup (1 run)

| Run | Avg (µs) | p50 (µs) | p99 (µs) | Aggregate (GB/s) |
|-----|----------|----------|----------|-------------------|
| 1   | 5814.0   | 6039.6   | 9021.5   | 5.77              |

Note: At 8 clients, aggregate throughput DROPS vs 4 clients (5.77 vs 7.83 GB/s).
This suggests contention/saturation — the pipeline becomes the bottleneck under concurrency.

## Hot Lookup (reference — not pipeline-dependent)

| Clients | Avg (µs) | Aggregate (GB/s) |
|---------|----------|-------------------|
| 1       | 268-319  | 13-16             |
| 4       | 836-856  | 19.6-20.1         |
| 8       | 1649.6   | 20.3              |

Hot lookups scale well to ~20 GB/s aggregate (memory-tier → GPU, no SSD reads).
Cold/Hot ratio: 2.3-3.5x depending on clients and eviction pressure.

## Key Observations

1. **Variance is higher than expected** — 10% CoV at 1 client driven by tail events
2. **Aggregate throughput peaks at 4 clients** (~7.9 GB/s) and drops at 8 (~5.8 GB/s)
3. **Hot lookup baseline** confirms pipeline is bottleneck: 20 GB/s aggregate for GPU DMA alone
4. **Current pipeline** (ZERO_COPY_DEPTH=16, 2 streams) leaves headroom: 4.9 GB/s vs 20 GB/s hot path
5. **Eviction overhead visible** — the p50-to-avg gap shows some lookups trigger eviction before read

## Notes for Bakeoff

- Use p50 cold latency as primary score (more stable than avg which is tail-skewed)
- OR: increase --num-objects to saturate memory-tier upfront, eliminating eviction variance
- Server was running since May 22 — rebuild before bakeoff to ensure clean state
- gpu-bb-vs-p2p micro-benchmark cannot run while server holds VFIO devices
