Validation passes. Here's a summary of the experiment findings:

---

## Iteration 2 Complete — All Arms Confirmed

**Research question answered:** The 2x latency gap between bounce and P2P is caused by the **copy phase**, not the NVMe read phase.

### Key measurements

| Phase | Bounce | P2P | Ratio |
|-------|--------|-----|-------|
| NVMe read (read_us) | ~790 µs | ~710 µs | **1.1x** (≈equal) |
| Memory copy (copy_us) | ~819 µs H2D | ~114 µs D2D | **7.2x** |
| Total latency | ~2.6 ms | ~1.3 ms | ~2x |

### New principles extracted

- **RP-3**: H2D cudaMemcpy is ~4.9 GB/s vs D2D ~35 GB/s for 32×128 KiB on A30 — a **7x intrinsic ratio**, larger than the >1.5x prediction. The 2x end-to-end gap is a *diluted* signal because the shared NVMe read phase (~700-790µs) accounts for half of total latency in both modes.
- **RP-4**: NVMe DMA read time is within 10% whether targeting host DRAM or GPU BAR1 — confirming the DMA target choice is not the differentiator.
- **RP-5**: The NVMe read phase (~700µs for 4MB) is a latency floor that caps pipelining gains. Iteration 3 should test CUDA stream overlap to break this floor.

**Ablation confirmed** the copy ratio (6.8x without NVMe reads vs 7.2x full-path) — no NVMe-copy interference effect exists.