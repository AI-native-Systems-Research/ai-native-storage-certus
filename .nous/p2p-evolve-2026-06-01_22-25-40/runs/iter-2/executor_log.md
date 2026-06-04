Validation passes.

## Summary

**Experiment complete.** Key findings from iteration 2 (GPU sync stall elimination):

1. **h-main (REFUTED)**: Removing the periodic GPU sync at QD16/2-streams produced NO measurable throughput improvement (3.76 vs 3.78 GB/s baseline). The GPU sync is not the bottleneck at low queue depths — the NVMe device is already underutilized with only 16 in-flight commands.

2. **h-ablation (PARTIALLY CONFIRMED)**: QD64 + 4 streams + sync removal achieved 4.59 GB/s — marginally above iter-1's best of 4.53 GB/s (+1.3%). The sync removal adds negligible benefit on top of the deeper queue/more streams. The dominant factor remains NVMe queue depth.

3. **h-robustness (PARTIALLY CONFIRMED)**: Sync interval 64 produced 3.90 GB/s — matching h-main (no improvement) as predicted, confirming the two approaches are equivalent. The sync isn't the bottleneck.

**New principles extracted:**
- RP-4: GPU sync removal has no effect at QD16 (high confidence)
- RP-5: Even at QD64, sync removal adds at most ~1.3% (medium confidence)
- RP-1 updated: Queue depth + stream count confirmed as the dominant levers