# Sync Apply Report — extent-manager (component-sync-specs)

**Date**: 2026-09-01T22:59:04Z
**Commit (pre-apply)**: 33bddaba
**Based on**: `proposals.json` / `drift-report.json` (2026-09-01)
**Backups**: `.specify/sync/backups/20260901T225904Z/{spec,plan,tasks}.md`
**Scope**: spec docs only — no `.rs` source modified, no cargo run.

## Outcome

| Category | Count |
|----------|-------|
| BACKFILL applied | 2 (FR-030, FR-016) |
| Plan-doc fixes applied | 2 (plan.md) + 4 (tasks.md) |
| ALIGN tasks generated | 0 |
| UNSPECCED backfilled | 0 |
| HUMAN_DECISION | 1 resolved (FR-030 → correct prose, per user) |

## Specs Updated (BACKFILL — applied directly)

| Requirement | File | Change type | Detail |
|-------------|------|-------------|--------|
| Header | spec.md | Modified | `**Updated**` date → 2026-09-01; added a "Last Synced 2026-09-01" note block. |
| FR-030 | spec.md | Modified | Removed the "(and in the format path)" claim from the prose. Flush is issued only after checkpoint writes (`lib.rs:308-310`); `format()` writes the superblock with no flush (`lib.rs:496-498`), matching `README.md:66` and `Cargo.toml:24-27`. User chose "correct spec prose" over adding a format-path flush. |
| FR-016 | spec.md | Modified | Replaced the stale "docs incorrectly state 'five minutes' / should be corrected" remediation note with a plain statement of the 30 s default; that correction had already landed (`iextent_manager.rs:205`, `README.md:13`). |
| Component Structure diagram | plan.md | Modified | `checkpoint_interval_ms: AtomicU64 (default 5000)` → `checkpoint_timer_state: Arc<CheckpointTimerState>` (Mutex<Option<Duration>> + Condvar + shutdown) @ 30 s default. |
| Source tree diagram | plan.md | Modified | `superblock.rs … (v5)` → `(v6)` (`FORMAT_VERSION = 6`). |
| Completed tasks | tasks.md | Modified | `v5 / CERTUSV5` → `v6 / CERTUSV4`; `CERTUSV5` → `CERTUSV4` (2 further sites). Magic is `0x4345_5254_5553_5634` = "CERTUSV4", version 6. |
| Open tasks | tasks.md | Modified | Marked the "runtime-configurable checkpoint interval" decision resolved — exposed as `set_checkpoint_interval` (FR-027), held in `CheckpointTimerState` not `AtomicU64`. Fixed `cargo bench -p extent-manager-v2` → `-p extent-manager` (correct crate name). |

## Align Tasks Generated

None — no ALIGN items. The one human-decision item (FR-030) was resolved as a
doc correction (BACKFILL) at the user's direction; no code change was made.

## Verification

- No `.rs` files modified; no cargo invoked (per task constraints).
- `grep` confirms no remaining `CERTUSV5`, `v5`, `AtomicU64 (default 5000)`, or
  "(and in the format path)" strings outside the spec's historical sync-note trail.
- Remaining `v2/` and "five minutes" hits are inside dated "Last Synced" notes
  (accurate historical description of prior state) — left intentionally.
- FR-030's implementation-status note already scoped the flush call site to the
  checkpoint path, so it is now fully consistent with the corrected prose.

## Notes

This pass closes the docs items the 2026-08-20 apply-report explicitly deferred
("`AtomicU64` checkpoint-interval wording and `CERTUSV5`/`v5` strings in
plan.md/tasks.md … a future docs pass can address them").
