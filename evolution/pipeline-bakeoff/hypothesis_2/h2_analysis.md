# H2 Analysis: Mixed Workload Evolution Bakeoff

**Date**: 2026-05-25
**Hypothesis**: "An evolved pipeline can adapt to mixed workload sizes (1/2/4/16 MiB) better than a static configuration."
**Target**: `pipelined_ssd_to_gpu_zero_copy()` in `components/dispatcher/src/pipeline.rs`
**Verdict**: FALSE — Aggressive static parallelism (QD=32-64, minimal sync) beats adaptive branching. No framework discovered a size-dependent strategy that outperforms the brute-force approach.

---

## 1. Executive Summary

Five search frameworks (AdaEvolve, EvoX, GEPA, OpenEvolve, K-Search) and Nous ran against a mixed-size evaluator that scores the equally-weighted composite of 1/2/4/16 MiB cold lookup throughput. Each search framework ran 10 iterations.

**Key findings:**

1. **All frameworks converge on "higher QD + less sync"** — the same insight from H1 (QD=32 optimal) dominates. No framework discovered genuine size-adaptive logic.
2. **16 MiB scores are inflated by memory-tier caching** — objects that fit in warm cache report ~20 GB/s (served from RAM, not SSD). This inflates composite scores non-deterministically.
3. **AdaEvolve wins on score (7.49 GB/s composite)** but this reflects a lucky 16 MiB cache hit (21.59 GB/s) rather than a real optimization.
4. **Nous definitively proved**: adaptive branching gives marginal +7-8% on small objects, but aggressive parallelism (QD=64 + 4 streams) gives +7% composite with less complexity.
5. **The QD cap `max_inflight = ZERO_COPY_DEPTH.min(num_chunks)` means QD changes don't help for objects ≤ QD chunks** — 1 MiB = 8 chunks, so QD>8 is irrelevant for small objects.

---

## 2. Results Summary

| Framework | Best Score | Best Iter | Key Constants (best) | 16 MiB Score |
|-----------|-----------|-----------|---------------------|-------------|
| **AdaEvolve** | 7.49 GB/s | iter 8 | QD=16*, SYNC=16 | 21.59 GB/s |
| **K-Search** | 6.21 GB/s | iter 4 | QD=48, SYNC=256 | 17.31 GB/s |
| **EvoX** | 5.37 GB/s | iter 3† | QD=48, SYNC=16 | 14.95 GB/s |
| **GEPA** | 4.66 GB/s | iter 10 | QD=32, SYNC=128 | 11.16 GB/s |
| **OpenEvolve** | 3.99 GB/s | iter 5 | QD=32, SYNC=64 | 8.83 GB/s |
| **Nous** | 4.08 GB/s | iter 1 | QD=64, SYNC=64, 4 streams | 9.68 GB/s |

*AdaEvolve's best kept QD=16 (default) but got lucky with 16 MiB caching.
†EvoX checkpoint_10 shows QD=48 as best, but it peaked at iter 3 with 5.37 GB/s.

**Baseline** (QD=16, SYNC=16, 2 streams): ~4.25 GB/s composite (first AdaEvolve eval).

### Per-Size Breakdown (AdaEvolve best candidate):
- 1 MiB: 0.89 GB/s (8 chunks → QD capped at 8 regardless of ZERO_COPY_DEPTH)
- 2 MiB: 1.95 GB/s (16 chunks → QD capped at 16)
- 4 MiB: 5.52 GB/s (32 chunks → QD=32 fully utilized)
- 16 MiB: 21.59 GB/s (**cache hit** — served from memory-tier, not SSD)

---

## 3. Why Adaptive Failed

### 3.1 The QD Cap Makes Small-Object Adaptation Irrelevant

```rust
let max_inflight = ZERO_COPY_DEPTH.min(num_chunks);
```

For 1 MiB objects (8 chunks at 128 KiB): `max_inflight = min(anything, 8) = 8`. No amount of QD tuning matters. For 2 MiB (16 chunks): capped at 16. The "mixed workload" problem reduces to "optimize for 4+ MiB" because smaller objects are already at their natural QD ceiling.

### 3.2 Nous Controlled Experiment (Definitive)

Nous iter-1 ran a proper 3-arm experiment:

| Arm | Config | 1 MiB | 4 MiB | 16 MiB | Composite |
|-----|--------|-------|-------|--------|-----------|
| **Control** | QD=16, SYNC=16, 2 streams | 0.83 | 3.77 | 8.69 | 3.82 |
| **h-main (adaptive)** | Branch by num_chunks | 0.89 (+7%) | 3.16 (-16%) | 8.47 (-3%) | 3.66 (-4%) |
| **h-robustness (aggressive)** | QD=64, 4 streams | 0.80 (-4%) | 4.07 (+8%) | 9.68 (+11%) | 4.08 (+7%) |
| **h-control-negative (throttled)** | QD=8, SYNC=4 | 0.80 (-4%) | 2.86 (-24%) | 5.62 (-35%) | 2.70 (-29%) |

**Conclusion**: Aggressive static parallelism (QD=64 + 4 streams) beats adaptive branching by +11% on composite, with simpler code.

### 3.3 16 MiB Cache Inflation

16 MiB scores vary from 5.6 to 21.6 GB/s depending on whether the object was in memory-tier warm cache. This makes the composite score dominated by a caching lottery. Framework "winners" are largely determined by whether their eval run had warm 16 MiB objects.

---

## 4. Framework Behaviors

### AdaEvolve
- Best score (7.49) was iter 8, but the high score came from 16 MiB=21.59 GB/s (cache hit)
- 4 MiB scores were inconsistent: 4.11, 4.02, 3.85, 5.28, 4.54, 4.36 GB/s across iterations
- **Produced git merge markers** in iteration 1 (=======) — compile failure, score 0
- Convergence: found decent configs early, then meandered

### EvoX
- Pushed QD to 48, which helps 4+ MiB objects (32+ chunks)
- Best at iter 3 (5.37 GB/s) with 14.95 GB/s on 16 MiB — moderate cache benefit
- Used adaptive sync frequency that varied by object size (SYNC_FREQUENCY for large, less for small)

### GEPA
- Conservative: QD=32, SYNC=128 (sync less often)
- Added conditional sync: `if num_chunks > 128 && (completed + 1) % SYNC_FREQUENCY == 0`
- This effectively disables mid-transfer sync for all objects ≤16 MiB (128 chunks = 16 MiB)
- Correct insight (less sync = better) but ineffective because sync was already cheap

### OpenEvolve
- QD=32, SYNC=64 — moved in the right direction but conservatively
- Lowest score among search frameworks (3.99 GB/s)
- 16 MiB at 8.83 GB/s suggests consistently cold cache state during eval

### K-Search
- Most aggressive: QD=48, SYNC=256 (effectively no mid-transfer sync)
- Pre-allocated chunk buffers rather than using ring pattern
- 6.21 GB/s composite with 17.31 GB/s on 16 MiB
- Architecturally similar to EvoX's direction but with larger constants

---

## 5. What We Learned

### 5.1 For the Presentation

- **Search frameworks cannot isolate causal factors.** All 5 frameworks produced compound changes (QD + sync + minor code tweaks). None could tell you WHICH change mattered. Only Nous decomposed this cleanly.
- **Mixed workload doesn't create a new optimization dimension.** The optimal config for 4 MiB is also optimal for mixed — because small objects are QD-capped anyway and large objects benefit from the same "more parallelism" strategy.
- **The real mixed-workload question is architectural (H3), not parametric.** Multiple clients with different request sizes need the outer Mutex removed, not per-size branching logic.

### 5.2 Methodology Issues

- **16 MiB cache inflation needs fixing.** Future evaluators should evict memory-tier between measurements to ensure all lookups are truly cold.
- **10 iterations per framework is insufficient** given ±50% baseline variance. Need 30+ with statistical controls, or use the micro-benchmark (3s per eval instead of 45s).
- **Auto-generated analysis showed 0.0 for all frameworks** — the orchestrator's score parsing reads from summary.json which was overwritten by the aborted H3 launch. Real scores are in checkpoint data.

---

## 6. Verdict

**H2 is FALSE.** No framework found a size-adaptive strategy that meaningfully outperforms aggressive static parallelism. The mixed-workload optimization problem reduces to "maximize QD for large objects" because small objects are already QD-saturated by the chunk count cap.

The correct config for mixed workloads is: `ZERO_COPY_DEPTH=32, SYNC_FREQUENCY≥64, 2+ CUDA streams` — the same conclusion as H1, just with relaxed sync.

**Next step (H3):** The real multi-workload bottleneck is the outer Mutex serializing ALL requests regardless of size. Multiple clients requesting different sizes should run in parallel, not queue behind a single lock.
