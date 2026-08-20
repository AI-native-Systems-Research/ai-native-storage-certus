# Sync Apply Report — extent-manager (Phase B)

**Date**: 2026-08-20
**Based on**: `proposals.json` (Phase B) / `drift-report.json` (2 drifted)
**Backups**: `.specify/sync/backups/specs/001-extent-manager-v2/spec.md.bak`,
`.specify/sync/backups/specs/001-extent-manager-v2/plan.md.bak`
**Scope**: spec docs only — no `.rs` source modified, no cargo run.

## Outcome

| Category | Count |
|----------|-------|
| BACKFILL applied | 2 |
| ALIGN tasks generated | 0 |
| UNSPECCED backfilled | 0 |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |

## Specs Updated (BACKFILL — applied directly)

| Requirement | File | Change type | Detail |
|-------------|------|-------------|--------|
| Header | spec.md | Modified | `**Updated**` date → 2026-08-20. |
| Top Sync note | spec.md | Modified | Added a "Last Synced 2026-08-20" block; removed the stale FR-030 "does not compile / drafted / queued / add CI job" text and the "Informational: plan.md still references block_device/v2" bullet (now resolved). |
| FR-030 | spec.md | Modified | Parenthetical status changed from "does not yet compile" to "implemented and building", citing `flush()`/`FlushSync`/`FlushDone` and their locations. |
| plan.md-layout-refs | plan.md | Modified | Dropped the `block_device` data-device receptacle from the "Storage" block and Component Structure list (noted data device is caller-owned per FR-036); re-rooted the source tree from `components/extent-manager/v2/` to `components/extent-manager/`; added a Last-Synced 2026-08-20 note to the plan header. |

## Align Tasks Generated

None — no ALIGN items this phase. (The existing `align-tasks.md` from the 2026-08-07
sweep is historical and left untouched; the FR-030 fix it drafted has since landed,
which is exactly what P1 backfills.)

## Unspecced Backfilled

None.

## Resolved

None.

## Verification

- No `.rs` files modified; no cargo invoked (per task constraints).
- `grep` confirms no remaining stale `block_device` receptacle or `extent-manager/v2/`
  references in plan.md (only the interface filename `iblock_device.rs` and the
  descriptive sync note remain).
- `grep` confirms FR-030 no longer carries a "does not compile / drafted on branch"
  status.

## Notes / Out of Scope

The 2026-08-07 informational list also flagged `AtomicU64` checkpoint-interval wording
and `CERTUSV5`/`v5` strings in plan.md/tasks.md/README.md. These are **not** in the
2026-08-20 drift report's two items and were left unchanged to keep this backfill
surgical; a future docs pass can address them.
