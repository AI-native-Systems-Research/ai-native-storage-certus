Validation passed. Here's a summary of the experiment results:

## Summary

**All three hypotheses were REFUTED.**

### Key findings:

**H-main (BatchSubmit + deferred GPU DMA): REGRESSION**
- cold_16384KiB mean: 12,674 us vs baseline 9,815 us (+29% worse)
- min latency: ~5,000-5,800 us vs baseline ~2,700 us (doubled — the SSD floor worsened)
- throughput: 1,262 MB/s vs baseline 1,633 MB/s (-23%)
- Concentrating all 128 reads on a single depth-256 qpair **hurts** this SSD hardware (7x Intel NVMe drives), contrary to the hypothesis

**H-ablation (BatchSubmit + per-completion GPU DMA): NEUTRAL at 16 MiB**
- cold_16384KiB mean: 10,243 us vs baseline 9,815 us (within noise)
- BatchSubmit alone (without deferred GPU DMA) has no effect on 16 MiB throughput
- Unexpected finding: cold_128KiB improved 3-4x consistently (775 us vs 2,900 us) — suggests different qpair selection for small BatchSubmit vs individual ReadAsync

**H-control-negative (at 128KiB): violated — some improvement observed**
- Shows that BatchSubmit(1 op) is NOT equivalent to individual ReadAsync at the qpair-routing level

### New principles extracted:
- **RP-21**: BatchSubmit qpair routing differs from per-command routing even at N=1, producing unexpected 128KiB speedup
- **RP-22**: Deferred GPU DMA + single deep qpair concentration hurts 16 MiB throughput by ~29% and doubles the min latency floor — NVMe/GPU overlap elimination costs more than channel overhead reduction saves