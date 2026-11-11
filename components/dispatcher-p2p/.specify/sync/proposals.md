# Drift Resolution Proposals — dispatcher-p2p

Generated: 2026-11-11T21:14:07Z · Branch `copilot/fix-dispatcher-lba-issues` · HEAD `adacb8ce1` · Mode: interactive

| Resolution Type | Count |
|---|---|
| Backfill (Code → Spec) | 1 |
| Align (Spec → Code) | 0 |
| Human Decision | 0 |
| New Specs | 0 |

## P-P2P1: FR-037 (new)

**Direction**: BACKFILL · **Confidence**: MEDIUM

- Spec says: (silent) — no LBA formula stated, so no contradiction
- Code does: same partition-relative rule as dispatcher FR-064 on promote_and_serve, write-through, P2P cold batch jobs, promote_to_memory_tier and the DRAM backfill worker
- Evidence: `src/lib.rs:614`, `src/lib.rs:919`, `src/lib.rs:1243`, `src/lib.rs:1288`, `src/lib.rs:1558`, `src/lib.rs:1605`, `src/lib.rs:2082`, `src/lib.rs:2948`
- Note: The approval preview called this FR-064. The p2p spec's own numbering ends at FR-036 (its FR-063 mention cross-references dispatcher), so the requirement was applied as FR-037.

**Action**: [x] Approve — Approved interactively by the user in /component-sync-specs --interactive, 2026-11-11.
