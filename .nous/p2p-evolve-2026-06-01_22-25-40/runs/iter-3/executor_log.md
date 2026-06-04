Validation passes.

## Summary

**Iteration 3 complete.** Key findings:

1. **h-main (QD32 × 4 threads × 4 streams): CONFIRMED** — Cold lookup improved from 3.63 GB/s baseline to 5.59 GB/s average (+54%), exceeding the 4.5 GB/s prediction.

2. **h-ablation (QD64 × 2 threads × 2 streams): PARTIALLY CONFIRMED** — Achieved 5.99 GB/s, *outperforming* h-main by 7%. The prediction that it would be below h-main was wrong. Queue depth alone is the dominant lever; adding threads introduces actor poll-loop contention.

3. **h-robustness (single drive, QD32 × 4 threads): PARTIALLY CONFIRMED** — Throughput above 3.0 GB/s on successful iterations but sporadic ENOMEM (rc=-12) errors at aggregate QD128 per controller, confirming RP-3 extends to 128 KiB chunks.

**New principles extracted:**
- **RP-1 updated**: Batch path QD increase gives ~56% throughput improvement (larger than the 21% seen on single-object path)
- **RP-3 updated**: ENOMEM boundary is ~QD96-128 per controller regardless of chunk size
- **RP-6 (new)**: More threads per drive hurts throughput by ~7% due to actor contention
- **RP-7 (new)**: 4 CUDA streams provide no benefit over 2 at current throughput levels

**Optimal configuration: QD64, 2 threads per drive, 2 streams** (h-ablation) — the simplest change yields the best result at 6.0 GB/s.