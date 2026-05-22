Validation passes. Here's a summary of the design:

**Experiment designed and validated for h8-dispatcher-p2p iteration 1.**

Key findings from probes:
- **P2P warm (pre-pinned GDRCopy):** 3002 MB/s, 1.33ms avg latency
- **Bounce (host DRAM staging):** 1045 MB/s, 3.83ms avg latency
- **P2P is ~2.9x faster** — the hypothesis that bounce is faster appears false

The bundle has 3 arms:
1. **h-main:** Confirms P2P warm > bounce with statistical rigor (20 iterations)
2. **h-control-negative:** P2P cold (per-request pin/unpin) tests whether the amortized setup is what creates the advantage
3. **h-robustness:** Tests at 64 KiB chunks (64 commands vs 32) to check if the advantage holds with doubled command count

All artifacts written to `.nous/h8-dispatcher-p2p/runs/iter-1/` and validation passes.