# Spec-Sync Phase B — Apply Report — block-device-filesys

**Generated**: 2026-08-20 (Phase B spec-sync)
**Spec edited**: `specs/001-block-device-filesys/spec.md`
**Backup**: `.specify/sync/backups/specs/001-block-device-filesys/spec.md.bak`
**Source changes**: none (`.rs` files untouched; cargo not run)

## Result counts

| Category | Count |
|---|---|
| BACKFILL applied | 1 (FR-015) |
| BACKFILL-UNSPECCED | 1 (FR-023, new) |
| ALIGN tasks | 0 |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |

## Specs Updated

| Requirement | Change type | Description |
|---|---|---|
| FR-015 | BACKFILL (reworded) | Softened: `create()`'s doc example documented as intentionally ` ```ignore ` (illustrative-only, would create a real file on disk); only `DeviceConfig::new` is the runnable/compiled example. Added a 2026-08-20 correction note. |
| FR-023 | BACKFILL-UNSPECCED (new FR) | Documents reserved crate-private config mutators `set_file_path`/`set_block_size`/`set_num_blocks` (`pub(crate)`, `#[allow(dead_code)]`, unused) as intentional and explicitly outside the public API / functional contract. |
| Metadata | Updated | `Last Synced` line updated to 2026-08-20 recording this Phase B run; prior 2026-08-07 note retained. |

## Align Tasks Generated

None. The single drift item is spec-overclaim against intentional code (not a behavioral bug); see `align-tasks.md`.

## Unspecced Backfilled

| Feature | Location | Disposition |
|---|---|---|
| `set_file_path` / `set_block_size` / `set_num_blocks` internal setters (`pub(crate)`, `#[allow(dead_code)]`) | `src/lib.rs:95-109` | Backfilled as FR-023 — reserved internal mutators, not public API, outside functional contract, no acceptance scenario. |

## Resolved

None.

## Notes

- Faithful-to-code stance: FR-015 was aligned to the code's intent rather than proposing a code change to make the example runnable, because a runnable `create()` doctest would have on-disk side effects. This supersedes the earlier (2026-07-22) FR-015 "add doc examples" align direction.
- FR-023 records intent only; it does not mandate removing the dead code (out of scope — no `.rs` edits). It flags that promotion to public API would require a spec change plus set-time validation (fields are only validated at `initialize()` via `DeviceConfig::new`).
