All five phases complete. Validation passes. Here's the summary:

---

## Experiment Complete — Validation PASSED

**h-main (Pipelining): CONFIRMED** — 2.02x speedup
- Sequential v0: **19,502 us/obj** (0.22 GB/s)  
- Pipelined v0: **9,659 us/obj** (0.43 GB/s)  
- Dominant mechanism: elimination of 32× per-chunk `DmaBuffer::new()` allocations via pre-allocated `cudaHostAlloc` ping-pong buffers. GPU copy hiding is secondary.

**h-control-negative (4 KiB single chunk): REFUTED** — prediction was "no improvement"
- Pipelined still 16% faster (1,466 vs 1,749 us) due to buffer pre-allocation, even with no overlap opportunity.

**h-robustness (P2P direct DMA): REFUTED** — P2P is not faster
- P2P: **20,136 us** vs sequential **19,502 us** — essentially identical, confirming GPU BAR1 write bandwidth does not compensate for the eliminated H2D copy at NODE-level topology.

**New principles extracted:**
- **RP-9**: Per-chunk SPDK allocation is the dominant latency component in v0 (not GPU copy)
- **RP-10**: P2P via GPU BAR1 is not faster than host-bounce at NODE-level PCIe topology  
- **RP-11**: `cudaMemcpyAsync` benefits latency even for single-chunk transfers