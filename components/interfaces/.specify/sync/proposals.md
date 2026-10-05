# Drift Resolution Proposals — interfaces

Generated: 2026-10-05  
Branch: `fix/poller-cpu-placement-logging`  
Based on: drift-report 2026-10-05

| # | Requirement | Direction | Severity | Location |
|---|---|---|---|---|
| 1 | DispatcherConfig::poller_base_cpu doc comment | ALIGN (doc) | minor | `components/interfaces/src/idispatcher.rs:46-56` |

### 1. DispatcherConfig::poller_base_cpu doc comment — ALIGN (doc)

- **Spec said:** When `None`, each drive's actor falls back to the first available CPU in its NUMA node (all drives on the same node would share that core).
- **Code does:** The dispatchers assign NUMA-local cores round-robin, excluding each node's first two; block-device fallback / unpinned cases as in dispatcher FR-011.
- **Note:** Pre-existing doc drift; user chose to fix in this PR and re-stamp all components.
- **Proposed / applied:** Doc comment corrected. FR-018 lists `poller_base_cpu` by name only, so no spec text change was needed.
- **Confidence:** HIGH — **Approved** (interactive, 2026-10-05)
