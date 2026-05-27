# Pipeline Bakeoff Results — concurrent evaluator

Date: 2026-05-26 00:39
Evaluator mode: concurrent
Baseline (current dispatcher, single drive, 4 MiB): **3.59 GB/s**
P2P reference (gpu-bb-vs-p2p): **3.4 GB/s**
Raw NVMe ceiling (QD=32): **5.28 GB/s**

## Summary Table

| # | Framework | Iters | Best (GB/s) | Mean (GB/s) | vs Baseline | Wall Time | Status |
|---|-----------|-------|-------------|-------------|-------------|-----------|--------|
| 1 | adaevolve        |   0/10  |   0.0000    |   0.0000    | +  0.0%     |     769s  | OK |

## Sample Efficiency (iterations to reach 90% of best)


## Cost / Efficiency

| Framework | Wall Time | Iters/hour | Seconds/iter |
|-----------|-----------|------------|--------------|
| adaevolve        |     769s |      4.7   |      768.6   |

## Hypothesis Assessment

H3: 'Evolved pipeline + service-level changes can improve multi-client concurrent throughput beyond current Mutex-bound architecture'

- Winner: **adaevolve** at 0.0000 GB/s
- Beats baseline (3.59 GB/s) by >10%: **NO** (-100.0%)
- Matches/exceeds P2P (3.4 GB/s): **NO**
- Approaches NVMe ceiling (5.28 GB/s): 0%

## Correctness Flags (requires manual verification)

**WARNING**: The evaluator measures throughput but does NOT verify data integrity.
Frameworks may achieve high scores by corrupting the data path. Check each winner for:

| Flag | What to look for |
|------|-----------------|
| Removed DMA copy | Skipping `cuda_memcpy_h2d_async` or `copy_to_gpu_async` calls |
| Zeroed output | Writing zeros instead of actual NVMe data to GPU |
| Skipped NVMe reads | Not issuing `submit_read` for all chunks |
| Broken sync | Removing stream synchronization (data race → incomplete transfers) |
| Truncated transfer | Only transferring first/last chunk, skipping middle |
| Hardcoded return | Returning `Ok(())` without doing actual work |

**Action required**: For each framework's best program, diff against initial_program.rs
and verify the data path is intact. Run `certus-api-bench.py` with data verification
to confirm correctness before declaring a winner.
