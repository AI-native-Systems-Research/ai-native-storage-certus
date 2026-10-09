# Drift Resolution Proposals — dispatcher

Generated: 2026-10-08 · Branch: `opt/evolve-latency/s20261004-001925_PR507` · Mode: interactive

| # | Requirement | Direction | Decision |
|---|---|---|---|
| 1 | FR-039 step (2) | BACKFILL | Approved |
| 2 | US-11 scenario 2 (+ scenario 6) | BACKFILL | Approved |
| 3 | TouchCheck / bulk codec (shmq-dispatcher, connector) | NEW_SPEC | Skipped (no specs/ tree; out of scope) |

## 1. FR-039 step (2)

- Spec said: (2) for MemoryTier hits, issues per-region memcpy_h2d_async H2D copies on the device's warm stream and performs a single deferred stream_synchronize after all hot-path copies are issued.
- Code does: warm copies only queued; single warm stream_synchronize deferred until after the cold block (steps 4-5); warm read pins released only after it; then remote lookup (FR-045).
- Rationale: Intentional, tested optimization (ada87967): overlapping the warm H2D drain with the cold NVMe reads; pin lifetime extended to keep it safe.

## 2. User Story 11 scenario 2

- Spec said: hot entries are served inline without waiting for cold promotions to complete.
- Code does: hot H2D copies queued before cold prep, not waited on during cold promotion; one warm-stream sync after the cold block confirms them; new scenario 6 for the ordering.
- Rationale: Scenario wording predates the deferral; new scenario mirrors the existing ordering test.

## 3. TouchCheck opcode and bulk codec

- No specs/ tree in those components; outside this component's hash inputs. Skipped by user.
