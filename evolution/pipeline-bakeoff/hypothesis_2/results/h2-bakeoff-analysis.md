# H2 Bakeoff Analysis: Mixed-Workload Pipeline Optimization

## Hypothesis

"An evolved pipeline configuration can generalize across multiple object sizes (1/2/4/16 MiB), achieving better overall throughput than a single-size-optimized configuration."

## Setup

- **Evaluator:** Mixed-size evaluation — runs cold lookups at 1 MiB, 2 MiB, 4 MiB, and 16 MiB, equally weighted average
- **Baseline:** ~3.59 GB/s (single-size baseline, used as reference)
- **Target file:** `pipeline.rs` (single-file, ~407 lines)
- **Frameworks:** AdaEvolve, EvoX, GEPA, OpenEvolve, K-Search (Nous attempted, no results)
- **Iterations:** 7-10 per framework
- **Drive:** Single NVMe drive

## Results

| Framework | Best (GB/s) | Mean non-zero | vs Baseline | Compile % | Iters |
|-----------|-------------|---------------|-------------|-----------|-------|
| **AdaEvolve** | **7.49** | 4.35 | +109% | 75% (9/12) | 10 |
| K-Search | 6.21 | 4.61 | +73% | 82% (9/11) | 10 |
| EvoX | 4.76 | 4.30 | +33% | 43% (6/14) | 7 |
| GEPA | 4.66 | 4.06 | +30% | 93% (14/15) | 7 |
| OpenEvolve | 3.99 | 3.77 | +11% | 83% (10/12) | 10 |
| Nous | — | — | — | — | no result |

## What Worked

1. **AdaEvolve found a strong outlier** (7.49 GB/s) — this is 2× baseline and significantly above the 5.28 GB/s single-size NVMe ceiling. The mixed-size average can exceed single-size ceiling because 16 MiB objects benefit from longer sustained transfers with better drive utilization.

2. **K-Search again reliable** — 82% compile rate, second-best peak (6.21 GB/s), consistently good scores.

3. **GEPA most consistent** — 93% compile rate, very low variance (3.68-4.66 range), but conservative exploration means lower peak.

## What Didn't Work

1. **EvoX low compile rate** (43%) — again spent too much time on strategy evolution, not enough on program quality.

2. **OpenEvolve barely improved** — 11% over baseline despite 10 iterations. Its mutations were too conservative for the mixed workload.

3. **High variance in AdaEvolve** — the 7.49 GB/s outlier was never reproduced (next best was 5.37). This suggests either measurement noise or a fragile configuration that only works under specific conditions.

## Key Insight

The mixed workload rewards adaptive behavior: configurations that adjust pipeline depth or sync strategy based on transfer size. However, with a single `pipeline.rs` file and static constants, true adaptivity is limited. The winning solutions likely found a good compromise `ZERO_COPY_DEPTH` that works across all sizes.

The 7.49 GB/s outlier from AdaEvolve deserves investigation — it may have found size-dependent logic (different pipeline depth for different transfer sizes) or it may be a measurement artifact from favorable caching state during the 16 MiB test.

## Scores by Framework (all evaluations)

- **AdaEvolve:** 4.25, 0, 3.72, 5.37, 0, 4.24, 3.93, 4.02, 7.49, 4.27, 3.97, 3.77
- **K-Search:** 3.66, 3.92, 4.64, 6.21, 4.73, 0, 6.02, 0, 4.40, 3.94, 4.25
- **EvoX:** 4.64, 3.88, 3.88, 0, 4.25, 0, 0, 0, 4.76, 4.41, 0, 0, 0, 0
- **GEPA:** 3.68, 4.05, 3.91, 4.42, 4.21, 4.15, 4.10, 4.46, 3.97, 0, 4.00, 4.17, 4.08, 4.66, 3.93
- **OpenEvolve:** 3.77, 3.77, 0, 0, 3.81, 3.99, 3.77, 3.84, 3.41, 3.90, 3.92, 4.09

## Comparison with H1

| Metric | H1 (fixed, 4 MiB) | H2 (mixed, 1-16 MiB) |
|--------|-------------------|----------------------|
| Best overall | 5.60 GB/s (AdaEvolve) | 7.49 GB/s (AdaEvolve) |
| Winner | AdaEvolve | AdaEvolve |
| Most reliable | K-Search (100%) | GEPA (93%) |
| Worst performer | GEPA (3.26 min) | OpenEvolve (3.41 min) |

The mixed workload has higher variance and a higher ceiling (16 MiB objects transfer more efficiently), but is harder to optimize consistently.
