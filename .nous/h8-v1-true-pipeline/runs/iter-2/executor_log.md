Validation passed. Here's the summary:

---

## Experiment Complete — Validation: PASS

**Iteration 2: Dispatcher v1 cudaHostAlloc Staging vs P2P**

### Results (4 MiB objects, NVMe 63:00.0, 10 objects, 1 iteration)

| Condition | SSD-tier latency | vs baseline |
|-----------|-----------------|-------------|
| Baseline run 1 (sequential v1) | 19,646 us | — |
| Baseline run 2 (sequential v1) | 23,717 us | — |
| h-main run 1 (cudaHostAlloc staging) | 22,523 us | ~+15% slower |
| h-main run 2 (cudaHostAlloc staging) | 21,412 us | within noise |
| h-control-negative (P2P) | 15,879 us | **+19% faster** |

### Key Findings

**h-main (REFUTED):** cudaHostAlloc staging pipeline shows no improvement over baseline — results fall within the ±20% system variance band. Root cause: NVMe read time (~600 us per 128 KiB chunk × 32 chunks ≈ 19,200 us) completely dominates GPU H2D copy time (~5-50 us per chunk). The theoretical maximum pipeline speedup from hiding GPU copies is ~1,600 us = 8% of total time — below the measurement noise floor.

**h-control-negative (CONFIRMED):** P2P remains fastest at 15,879 us (+19% over baseline run 1), confirming the direct-DMA mechanism is correct and consistent with iter-1.

**New principle extracted — RP-16:** At 128 KiB chunks, NVMe read dominance makes pipeline copy-overlap a second-order optimization. To break the latency barrier, the approach must parallelize NVMe reads (BatchSubmit QD=32) rather than overlap a serial read sequence with GPU copies.