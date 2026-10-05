# Drift Resolution Proposals — dispatcher

Generated: 2026-10-05  
Branch: `fix/poller-cpu-placement-logging`  
Based on: drift-report 2026-10-05

| # | Requirement | Direction | Severity | Location |
|---|---|---|---|---|
| 1 | FR-011 | BACKFILL | moderate | `components/dispatcher/src/lib.rs:550-600, 1530-1539` |

### 1. FR-011 — BACKFILL

- **Spec said:** `poller_base_cpu` ... Defaults to `None` (OS scheduler decides).
- **Code does:** `None` selects NUMA-local automatic placement: round-robin over each drive's NUMA-node cores excluding the node's first two (this branch; previously global CPUs 0/1 were excluded); unresolvable node or node with <=2 cores -> no CPU passed, block device chooses; topology discovery failure -> warning, unpinned.
- **Note:** Pre-existing drift (the None path was already automatic on unstable), surfaced because this branch modifies that path.
- **Proposed / applied:** FR-011 rewritten for the None case; Last Synced header added.
- **Confidence:** HIGH — **Approved** (interactive, 2026-10-05)
