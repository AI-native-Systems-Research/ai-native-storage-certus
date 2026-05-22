Validation passed. 

## Summary

Designed the hypothesis bundle for **h8-v1-vs-p2p iteration 1** comparing dispatcher v1's pipelined bounce-buffer path against a direct SSD→GPU P2P DMA path.

**Key design decisions:**
- The P2P path skips memory-tier promotion entirely (reads NVMe directly into GPU via `prepare_memory_for_spdk` + sub-buffer DmaBuffer views)
- This eliminates 32 x `cudaMemcpy(128KiB, H2D)` from the critical path
- The control-negative arm tests at 4 KiB where `prepare_memory_for_spdk` setup cost should dominate the eliminated cudaMemcpy

**Artifacts written to** `.nous/h8-v1-vs-p2p/runs/iter-1/`:
- `problem.md` — problem framing with system interface, conditions, and success criteria
- `bundle.yaml` — h-main (4 MiB P2P vs bounce) + h-control-negative (4 KiB)
- `handoff_snapshot.md` — full code map and exploration context for executor/next designer