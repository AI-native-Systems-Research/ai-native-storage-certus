All five phases complete. Here's a summary of findings:

---

## Experiment Complete — Validation: PASS

### Results Summary

| Mode | Throughput | Avg Latency | read_us | copy_us | total_us |
|------|-----------|-------------|---------|---------|----------|
| Sequential bounce | 1440 MB/s | 2.78ms | 649 | 826 | 1475 |
| **Pipelined bounce** | **1764 MB/s** | **2.27ms** | 1204 | 112 | 1627 |
| Pipelined 2-stream | 1774 MB/s | 2.25ms | 1212 | 112 | 1630 |
| P2P warm | 3082 MB/s | 1.30ms | 656 | 114 | 771 |

### Arm Outcomes

- **h-main (pipelined bounce): PARTIALLY_CONFIRMED** — Direction correct (17% faster than sequential), but magnitude wrong. Predicted ~50% improvement, got 17%. The `cudaMemcpyAsync` truly works asynchronously on SPDK hugepages (copy_us: 826→112μs), but per-chunk `connect_client()` calls inflate the read phase to 1204μs vs 649μs for BatchSubmit.

- **h-control-negative: CONFIRMED** — Sequential bounce shows `total_us = read_us + copy_us` exactly (1475 = 649 + 826, 0% difference), confirming perfectly serial phases.

- **h-robustness: CONFIRMED** — 1-stream vs 2-stream: 1764 vs 1774 MB/s (0.6% difference). Single CUDA stream is not a bottleneck.

### Key Findings
1. SPDK hugepages work with `cudaMemcpyAsync` without `cudaHostRegister`
2. Per-chunk `connect_client()` overhead (~17μs each × 32 chunks = ~544μs) is the main bottleneck preventing the predicted 50% speedup
3. Pipelined bounce is fundamentally limited vs P2P: double PCIe bandwidth consumption means it cannot match P2P warm even with perfect pipelining
4. Next iteration should test reusing a single IBlockDevice channel across all pipelined reads (avoiding repeated `connect_client()`)