# Drift Resolution Proposals: eviction-policy-session-lists

Generated: 2026-10-31T04:54:40Z
Based on: drift-report from 2026-10-31T04:54:40Z (clippy commit `55a4ddd4`)

| Resolution Type | Count |
|---|---|
| Backfill (Code → Spec) | 1 |
| Align (Spec → Code) | 0 |
| Human Decision | 0 |

## B-1: specs/001-session-list-eviction/spec.md / Observability

**Direction**: BACKFILL (citation only)

- Spec says: `src/lib.rs:83-87,107-120`
- Code is now at: `src/lib.rs:83-87,149-162` (the `announced` field did not move; the announcement in `create_pool` moved by +42)

**Rationale**: 55a4ddd4 inserted lines above the cited code; the code is unchanged, so the code is authoritative.

**Confidence**: HIGH

**Action**: [x] Approve (rule: BACKFILL of code already on the branch)

## Not proposed

- The spec does not say that a poisoned lock panics. This is pre-existing and unchanged by `55a4ddd4`. Recorded in the drift report only.

Previous proposals: `backups/20261031T045440Z/proposals.md.prev`.
