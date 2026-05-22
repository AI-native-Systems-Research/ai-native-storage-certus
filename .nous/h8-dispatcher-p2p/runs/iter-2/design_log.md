Validation passes. All three artifacts are written and valid.

## Summary

**Iteration 2 design complete.** The hypothesis bundle decomposes iteration 1's P2P advantage into two orthogonal factors:

1. **h-main (P2P-seq vs bounce-seq):** Tests whether the P2P path alone (NVMe→GPU BAR1) is faster than bounce (NVMe→host DRAM→GPU) when both use the dispatcher's sequential ReadSync pattern. This is the key decision gate for dispatcher integration.

2. **h-robustness (P2P-batch vs bounce-batch):** Confirms iter-1's finding still holds, establishing the performance ceiling.

3. **h-ablation (bounce-batch vs bounce-seq):** Isolates the submission strategy contribution by comparing BatchSubmit vs sequential on the bounce path only.

**Code changes required:** Two new `TransferMode` variants (`P2pSeq`, `BounceSeq`) and corresponding handlers in `p2p_server.rs` that replicate the dispatcher's sequential ReadSync pattern against P2P and bounce targets respectively.

**Decision tree for iteration 3:** The results will tell us whether to implement P2P alone, BatchSubmit alone, or both in the dispatcher.