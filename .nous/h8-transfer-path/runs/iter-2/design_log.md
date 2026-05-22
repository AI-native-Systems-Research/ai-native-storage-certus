Design complete. Validation passes.

## Summary

**Iteration 2** decomposes the established 2x latency gap between bounce and P2P warm into its constituent phases:

- **h-main:** Adds `std::time::Instant` per-phase instrumentation to both `handle_bounce` and `handle_p2p`, reporting `read_us` and `copy_us` in the server response. Predicts the copy phase (H2D vs D2D) accounts for >60% of total time and is >1.5x slower in bounce, while NVMe read times are approximately equal.

- **h-ablation:** Adds `--skip-nvme` flag to remove NVMe reads entirely, isolating pure copy-phase performance. Confirms the H2D/D2D speed difference isn't an artifact of NVMe-copy PCIe bus interaction.

- **h-control-negative:** Uses the same instrumentation to verify NVMe read phases are equivalent between modes (within 20%), confirming DMA target doesn't matter for the NVMe controller at 128 KiB chunk size.

All artifacts written to `.nous/h8-transfer-path/runs/iter-2/` and campaign handoff updated.