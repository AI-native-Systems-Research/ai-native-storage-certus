# Drift Resolution Proposals — block-device-spdk-nvme

Generated: 2026-10-05  
Branch: `fix/poller-cpu-placement-logging`  
Based on: drift-report 2026-10-05

| # | Requirement | Direction | Severity | Location |
|---|---|---|---|---|
| 1 | FR-013 | ALIGN+BACKFILL | major | `components/block-device-spdk-nvme/src/lib.rs:229-250` |

### 1. FR-013 — ALIGN+BACKFILL

- **Spec said:** The actor service thread MUST be pinned to a core in the same NUMA zone as the NVMe controller device.
- **Code does:** Branch revision used `cpus().iter().nth(2)` for the fallback core, which returns None on a NUMA node with <=2 cores and left the actor UNPINNED (violates the MUST). Fixed: `.nth(2).or_else(|| first core)`.
- **Note:** Regression introduced by this branch, caught by this sync before stamping.
- **Proposed / applied:** Code fixed; FR-013 now documents set_actor_cpu precedence and the third-core / first-core fallback.
- **Confidence:** HIGH — **Approved** (interactive, 2026-10-05)
