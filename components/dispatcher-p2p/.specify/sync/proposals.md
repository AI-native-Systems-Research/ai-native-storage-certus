# Drift Resolution Proposals — dispatcher-p2p

Generated: 2026-10-05  
Branch: `fix/poller-cpu-placement-logging`  
Based on: drift-report 2026-10-05

| # | Requirement | Direction | Severity | Location |
|---|---|---|---|---|
| 1 | FR-027 (new) | BACKFILL | minor | `components/dispatcher-p2p/src/lib.rs:272-323, 894-903` |

### 1. FR-027 (new) — BACKFILL

- **Spec said:** (unspecced)
- **Code does:** Same poller placement policy as dispatcher FR-011, including this branch's skip-first-two-cores-per-node change.
- **Note:** Unspecced behaviour, modified by this branch.
- **Proposed / applied:** FR-027 added, cross-referencing dispatcher FR-011.
- **Confidence:** HIGH — **Approved** (interactive, 2026-10-05)
