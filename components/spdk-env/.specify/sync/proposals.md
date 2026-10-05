# Drift Resolution Proposals — spdk-env

Generated: 2026-10-05  
Branch: `fix/poller-cpu-placement-logging`  
Based on: drift-report 2026-10-05

| # | Requirement | Direction | Severity | Location |
|---|---|---|---|---|
| 1 | FR-022 (new) | BACKFILL | moderate | `components/spdk-env/src/env.rs:91-102, 115-139` |

### 1. FR-022 (new) — BACKFILL

- **Spec said:** (unspecced)
- **Code does:** init_spdk_env saves the caller's affinity (sched_getaffinity) before spdk_env_init and restores it (sched_setaffinity) after, undoing DPDK EAL's main-lcore pin so later-spawned threads do not inherit CPU 0. Best-effort.
- **Note:** New behaviour on this branch.
- **Proposed / applied:** FR-022 added; Last Synced header added.
- **Confidence:** HIGH — **Approved** (interactive, 2026-10-05)
