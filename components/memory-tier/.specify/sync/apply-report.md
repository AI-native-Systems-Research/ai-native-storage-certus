# Sync Apply Report — memory-tier

Applied: 2026-10-14T17:22:13Z
**Component**: memory-tier · **Spec**: `001-memory-tier` · **Mode**: BATCH
**Branch**: `fix/memory-tier-insert-track-panic` (base `78f60ab8`)
**Source**: `.specify/sync/proposals.json` (generated 2026-10-14T16:47:49Z); approved by Cornel Constantinescu, 2026-10-14
**Backup**: `.specify/sync/backups/20261014T172213Z/` (`specs/001-memory-tier/{spec,plan,tasks}.md`, plus the previous `align-tasks.md`, `apply-report.{md,json}` and `drift-report.md`)

No code was changed. `src/`, `components/interfaces` and `Cargo.toml` are untouched.

## Changes Made

### Specs Updated

| Proposal | Spec / file | Requirement | Change Type |
|----------|-------------|-------------|-------------|
| P1 | `spec.md` FR table | FR-030 | Added (refused `track` in `insert` → `NotInitialized`; PoolFull is checked first; nothing reserved or recorded; no panic) |
| P1 | `spec.md` US1 Test Coverage | `insert_fails_cleanly_when_policy_refuses_to_track` | Added |
| P8 | `spec.md` US1 Test Coverage | `peek_does_not_update_eviction_order` (FR-012) | Modified (the claim now says what the test actually checks) |
| P5 | `spec.md` FR table | FR-023 | Modified (initialized-flag check, with its exceptions listed) |
| P6 | `spec.md` FR table | FR-027 | Modified (which paths are measured; eviction counting) |
| P6 | `spec.md` Implementation Notes, 1st bullet | telemetry sentence | Modified |
| P7 | `spec.md` NFR table | NFR-005 | Modified (first-fit is O(k) linear; map ops O(log k)) |
| P10 | `plan.md` Spec-Sync Notes | sharding note | Superseded (marked SUPERSEDED 2026-08-20; the single `RwLock<Pool>` is authoritative) |
| P10 | `tasks.md:16,24,37,43` | shard/Creusot tasks | Superseded (`- [ ]` → `- [~] (superseded 2026-08-20: …)`) |
| P10 | `tasks.md:13` | test-count task | Modified (adds the regression test) |
| (skill step 2.5) | `spec.md` header | `**Last-Synced**` | Modified (2026-10-14) |

Notes on how each approval was applied:
- **P1**: approval note "spec.md parts only; keep NotInitialized (no new variant)". Only the two `spec.md` edits were made. FR-030 names `NotInitialized`. The decision point (a dedicated variant) was declined, so no interface ALIGN was generated for it.
- **P1 + P8** both edit `spec.md:41`, so they were combined as the proposal asks: P8's rewritten line comes first, followed by P1's new line.
- **P6**: its note says to use "that freed a slot" if P3 is applied. P3 was approved as an ALIGN task only, and the code is unchanged, so FR-027 keeps "for which the policy returned a victim", which matches the current code. The wording change is an acceptance criterion of the P3 task.
- **P10**: the exact plan.md/tasks.md text from the proposal. That text is itself the "superseded" metadata. `spec.md` already says the sharded design was dropped (its Spec-Sync Notes, 2026-08-20), so no further cross-reference was added.

### New Specs Created
- None. (P9 NEW_SPEC was not approved.)

### Implementation Tasks Generated
- 2 ALIGN tasks in `.specify/sync/align-tasks.md`: P3 (FR-013, `evict_next` race) and P4 (SC-3, touches after dropping the pool lock race with `clear`).
- 2 open items in the same file: P2 and P9, each quoting its approval note.

### Not Applied

| Proposal | Reason |
|----------|--------|
| P2 (interfaces doc, `imemory_tier.rs:33,66`) | Not approved: "deferred: changes components/interfaces, which re-stamps every component; separate PR". Recorded as an open item. |
| P9 (FR-031, disconnected receptacle) | Not approved: "open decision (A: errors instead of panics / B: document must stay connected); record in align-tasks.md". Recorded as an open item. |
| P3, P4 | Approved as ALIGN, so no spec or code edit was made; recorded as tasks. |

## Left as is (flagged for the next sync)
- The closing line of `spec.md` "Spec-Sync Notes", "No open items remain.", is now out of date: P3/P4 tasks and P2/P9 are open. No approved proposal covers that line, so it was not edited.
- `cargo test` was not run. This apply step edited only Markdown.

## Next Steps

1. Review the updated `spec.md`, `plan.md` and `tasks.md` (`git diff components/memory-tier/specs/`).
2. Commit: `git add components/memory-tier/specs/ components/memory-tier/.specify/sync/ && git commit -m "sync(memory-tier): apply approved drift resolutions"`.
3. Restamp the `spec_sync_*` frontmatter with `scripts/spec-sync-hash.sh` (the specs changed, so the digest moved).
4. Implement the P3/P4 tasks in `align-tasks.md`, each starting with a failing test. Decide on P9, and open the separate interfaces PR for P2.
