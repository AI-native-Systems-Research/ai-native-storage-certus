Validation passes. 

## Summary

Design artifacts written and validated for iteration 1 of the bounce-vs-P2P transfer path hypothesis:

- **`problem.md`** — Full problem framing with validated baseline results (bounce: 1544 MB/s / 2.59ms, P2P warm: 3064 MB/s / 1.31ms, P2P cold: 541 MB/s / 7.39ms), build commands including required `RUSTFLAGS`, and the `run_condition.sh` harness.

- **`bundle.yaml`** — Two-arm hypothesis bundle:
  - **h-main**: Predicts bounce is faster (testing the stated research question). The mechanism posits that NVMe DMA to host hugepages is faster than to GPU BAR1, offsetting the extra H2D copy.
  - **h-control-negative**: Predicts P2P cold is slower than P2P warm (validates GDRCopy setup overhead is real and the measurement framework detects differences).

- **`handoff_snapshot.md` + `handoff.md`** — Updated with validated baseline results, RUSTFLAGS requirement, PCI address (0000:62:00.0), and suggested next steps (per-phase instrumentation if h-main is refuted).