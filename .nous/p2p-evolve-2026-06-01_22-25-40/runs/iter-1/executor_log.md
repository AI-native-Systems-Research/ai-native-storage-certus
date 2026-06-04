## Summary

Experiment iteration 1 is complete. Key findings:

**h-main (QD64 + 4 CUDA streams): CONFIRMED** — Cold lookup throughput improved from 3.93 GB/s to 4.53 GB/s (+15.3%). Consistent across all 3 runs. The deeper NVMe queue and additional CUDA streams successfully overlap SSD I/O with GPU DMA transfers.

**h-ablation (512 KiB chunks): REFUTED** — Throughput *decreased* to 3.74 GB/s (-4.8%). Larger chunks reduce the number of pipeline segments (8 vs 32 for 4 MiB objects), starving the sliding-window pipeline of overlap opportunity.

**h-super-additivity (combined): REFUTED** — Throughput decreased to 3.63 GB/s (-7.6%) with occasional I/O errors (ENOMEM). The 512 KiB chunk size dominates negatively, limiting the pipeline to only 8 segments regardless of queue depth. The deep queue cannot be utilized when there are fewer segments than the queue depth.

Best result: **h-main at 4.53 GB/s, score 0.369** (vs baseline 0.327). For iteration 2, the h-main configuration should be adopted as the new baseline, and further optimization should focus on either multi-drive parallelism or pipeline architecture changes that maintain the 128 KiB chunk granularity while pushing even deeper parallelism.