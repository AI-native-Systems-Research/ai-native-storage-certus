# Sync Apply Report

Applied: 2026-10-14T18:11:40Z
Branch: `fix/dispatch-map-get-pool-id-panic` (HEAD `0ca07fca`)
Mode: batch
Based on: `.specify/sync/proposals.json` (2026-10-14), `.specify/sync/drift-report.json` (2026-10-14)
Approved by: Cornel Constantinescu, 2026-10-14
Backups: `.specify/sync/backups/20261014T181140Z/specs/001-dispatch-map/` (`spec.md`, `data-model.md`, `contracts/idispatch_map.md`)

No source code was changed: nothing under `src/`, `tests/` or `components/interfaces/` was edited.

## Changes Made

### Specs Updated

| Proposal | Spec | Requirement | File | Change Type |
|----------|------|-------------|------|-------------|
| P1 | 001-dispatch-map | FR-003 | `spec.md` (FR-003) | Modified (BACKFILL): lists `InvalidSize`, `NotInitialized`, `AlreadyExists` in check order; no entry recorded on error |
| P1 | 001-dispatch-map | FR-003 | `contracts/idispatch_map.md` (`create_memory_tier_entry` row) | Modified (BACKFILL): adds `NotInitialized`; no entry recorded on error |
| P2 | 001-dispatch-map | FR-024 | `spec.md` (FR-024) | Modified (BACKFILL): adds `NotInitialized`; no entry recorded on error |
| P2 | 001-dispatch-map | FR-024 | `contracts/idispatch_map.md` (`recover_extent` row) | Modified (BACKFILL): adds `NotInitialized`; no entry recorded on error |
| P3 | 001-dispatch-map | FR-017 | `spec.md` (FR-017) | Modified (BACKFILL): empty list (no panic) when no `IEvictionPolicy` is connected |
| P3 | 001-dispatch-map | FR-017 | `contracts/idispatch_map.md` (`oldest_keys` row) | Modified (BACKFILL): empty list when no `IEvictionPolicy` is connected |
| P4 | 001-dispatch-map | FR-012 | `data-model.md` (error table, `NotInitialized`) | Modified (BACKFILL): no longer "reserved"; returned by `initialize`, `create_memory_tier_entry`, `recover_extent` |
| P5 | 001-dispatch-map | FR-026 | `contracts/idispatch_map.md` (`try_evict_to_block` row) | Modified (BACKFILL): `ActiveReferences` for held refs; `InvalidState` only for no `ssd_offset` / not `MemoryTier` |
| P5 | 001-dispatch-map | FR-026 | `data-model.md` (error table, `ActiveReferences`) | Modified (BACKFILL): adds `try_evict_to_block` |
| P5 | 001-dispatch-map | FR-026 | `data-model.md` (error table, `InvalidState`) | Modified (BACKFILL): narrows the `promote_block_to_memory_tier` / `try_evict_to_block` cases |
| P10 | 001-dispatch-map | header / Last Synced | `spec.md` (new note after the 2026-10-09 note) | Added (SUPERSEDE): 2026-10-14 note that supersedes the earlier notes' "FR-012 open" statement; earlier dated notes kept as history |

11 text edits in 3 files. Each `old` text from `proposals.md` matched exactly once at the line the proposal gave, and was replaced with the `new` text verbatim.

SUPERSEDE handling (P10): the spec has one feature spec and no successor spec, so there is no `superseded_by` field to add. As the proposal specified, the superseding statement is a new dated **Last Synced** note; the old notes are unchanged.

### New Specs Created

None.

### Implementation Tasks Generated

4 approved ALIGN proposals recorded in `.specify/sync/align-tasks.md`. Each has the approval note "approved as an ALIGN task only: record in align-tasks.md, no code change in this PR".

| Proposal | Requirement | Task | Effort |
|----------|-------------|------|--------|
| P6 | FR-003 / US1-AS3 / Edge Cases (null pointer) | Existing task refreshed (line refs at `0ca07fca`, alternative recorded) | small |
| P7 | no-panics rule / FR-002 / FR-012 (stale pool id, `track(...).unwrap()` under the lock) | New task | medium |
| P8 | FR-012 (test for unbound `initialize()`) | New task; closes the last open criterion of the FR-012 task | small |
| P9 | FR-023 (`entry_size` `u32` overflow) | New task (includes the FR-023 spec follow-up) | small |

The existing FR-012 task in `align-tasks.md` now has a 2026-10-14 status line: two of its three criteria are met by `c877b74e`, and the remaining one (the `initialize()` test) is tracked by P8.

### Not Applied

| Proposal | Reason |
|----------|--------|
| — | None. All ten proposals are `approved: true`. No unapproved proposal has an `approval_note`, so `align-tasks.md` has no open items from unapproved proposals. |

## Notes

- `proposals.md` still says "Every proposal is `approved: false`" (it was written before approval). `proposals.json` is the approval record and was used as the input; neither file was changed by this apply.
- The `spec_sync_*` frontmatter in `drift-report.md` was not touched.

## Next Steps

1. Review the updated specs: `components/dispatch-map/specs/001-dispatch-map/{spec.md,contracts/idispatch_map.md,data-model.md}`.
2. Commit the spec and sync changes on this branch.
3. Implement the align tasks in `.specify/sync/align-tasks.md` (P6–P9) in a follow-up PR, then re-run `speckit-sync-analyze`.
