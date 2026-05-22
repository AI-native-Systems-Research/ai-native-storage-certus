Validation passed. Here's the summary:

---

## Experiment Complete — Iter-2 Results

**Validation:** `{"status": "pass"}`

### What was tested
BatchSubmit QD=32 (all 32 × 128 KiB NVMe reads submitted simultaneously) vs sequential QD=1 baseline for 4 MiB SSD-tier lookups through certus-server.

### Results

| Condition | Canonical (us) | Multi-run median (us) |
|-----------|---------------|----------------------|
| Baseline (sequential v0) | 27,746 | ~22,071 |
| h-main (BatchSubmit + async copies) | 28,030 | ~23,015 |
| h-ablation (BatchSubmit + sync copy) | 20,235 | ~24,218 |

### Key Finding: **h-main REFUTED**

BatchSubmit QD=32 provides **no measurable latency improvement**. All three conditions are statistically indistinguishable (±30-50% variance). The predicted 4-8x speedup (target 1,200-2,500 us) did not materialize.

**Root cause (RP-12):** The certus-server gRPC round-trip + `connect_client()` per-lookup overhead (~15-25ms) completely swamps NVMe read time (~300-800 us at QD=32). NVMe parallelism is real at the hardware level but contributes <5% of total latency — invisible in this benchmark architecture.

**h-ablation PARTIALLY_CONFIRMED:** Sync vs async GPU copy is indistinguishable (same regime problem — GPU copy ~200us is negligible against ~20ms total).

**Principle updates added:** RP-12 (gRPC overhead dominates NVMe time), RP-13 (async copy benefit negligible at this scale), revised RP-9 (DmaBuffer::new less dominant than iter-1 suggested when full gRPC overhead is included).

**Suggested iter-3:** To observe NVMe QD=32 benefit, bypass gRPC with persistent actor-level channels or a connection-pooling approach that avoids `connect_client()` per lookup.