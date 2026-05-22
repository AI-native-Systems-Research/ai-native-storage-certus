Validation passes. Let me provide a brief summary.

---

**Design complete for h8-v1-pinned iteration 2.** Validation passes.

**Summary:** The experiment is a 2×2 factorial design testing P2P vs bounce at two PCIe topology levels:
- **NODE** (same-NUMA): NVMe `63:00.0` → GPU0, both NUMA 0, different root complexes (`0000:60` → `0000:40`)
- **SYS** (cross-NUMA): NVMe `c2:00.0` → GPU0, NUMA 1→0, crosses Infinity Fabric (`0000:c0` → `0000:40`)

This isolates the AMD Infinity Fabric contribution to the 65% P2P penalty observed in iter-1. If NODE-level P2P shows a significantly smaller penalty ratio, the NUMA interconnect is confirmed as the dominant overhead factor. If the penalty persists at ~65%, cross-root-complex routing itself is the bottleneck and P2P is not viable on this hardware.