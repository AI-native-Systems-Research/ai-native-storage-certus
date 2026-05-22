All five phases complete. Here is a summary:

---

## Experiment Complete — h8-dispatcher-p2p Iteration 1

**Status:** `{"status": "pass"}` — validation passed.

### Results

| Condition | Avg Latency | Throughput | Seeds |
|-----------|-------------|------------|-------|
| Bounce 128K | 2.72 ms | 1469 MB/s | 3 |
| P2P warm 128K | 1.10 ms | 3638 MB/s | 3 |
| P2P cold 128K | 7.45 ms | 537 MB/s | 3 |
| Bounce 64K | 3.83 ms | 1048 MB/s | 3 |
| P2P warm 64K | 1.88 ms | 2125 MB/s | 3 |

### Arm Outcomes

- **h-main: CONFIRMED** — P2P warm is 2.47x faster than bounce. The original hypothesis (bounce is faster) is decisively falsified. Min-max ranges don't overlap across any of the 60 total observations.
- **h-control-negative: CONFIRMED** — P2P cold is 2.74x *slower* than bounce (7.45ms avg), confirming that pre-pinned GDRCopy staging is mandatory for P2P to deliver an advantage.
- **h-robustness: CONFIRMED** — At 64K chunks P2P warm still leads (2.04x) but the advantage narrows vs 128K (2.47x), consistent with the prediction of increased per-chunk D2D copy overhead.

### Key Principles Extracted (RP-1, RP-2, RP-3)

Pre-pinned GDRCopy BAR1 P2P delivers ~2.5x lower latency by eliminating the host-DRAM intermediate hop. Cold P2P negates this advantage entirely. Iteration 2 should implement P2P in `dispatcher/v1/src/pipeline.rs` using a pre-pinned staging pool.