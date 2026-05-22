Validation passes. Let me provide a brief summary.

---

**Design complete for iteration 2.** Validation passed.

**Summary:** Iter-1 confirmed pipelining works (17% faster than sequential bounce) but identified `connect_client()` per chunk as the bottleneck (adding ~544μs to the read phase). Iter-2 fixes this by reusing a single `ClientChannels` across all 32 pipeline iterations — sending individual `ReadAsync` commands on the shared SPSC channel (capacity 64) instead of creating 32 separate client sessions.

**Bundle arms:**
- **h-main:** Pipelined bounce with channel reuse — predicts total_us < 900μs, throughput > 2500 MB/s
- **h-ablation:** Same pipeline but with synchronous cudaMemcpy — isolates async overlap contribution
- **h-control-negative:** Sequential bounce unchanged — validates baseline stability
- **h-robustness:** P2P warm reference — confirms system state consistency