Validation passes. Here's the summary:

**Iteration 2 design complete.** The hypothesis shifts from pipeline micro-optimizations (which failed in iter-1 due to being below noise floor) to a structural NVMe command scheduling change:

- **h-main:** Submit all 128 chunk reads via a single `BatchSubmit` command (concentrating on one depth-256 qpair), then do GPU DMA in a batch after all reads complete. This eliminates 127 channel sends, all per-completion resubmission jitter, and qpair scattering.
- **h-ablation:** Same BatchSubmit but keep per-completion GPU DMA interleaving — isolates the qpair concentration effect.
- **h-control-negative:** Same code at 128 KiB (1 chunk) where BatchSubmit is functionally identical to baseline.

The key insight driving this design: the dispatcher's min latency (~2700 us) already matches/beats the reference, so the pipeline algorithm is fine. The problem is VARIANCE — and the most likely source is the 128 inter-thread synchronization points created by the per-completion resubmission pattern.